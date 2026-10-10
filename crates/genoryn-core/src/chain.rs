use crate::{
    block::{Block, Body, Header},
    codec::{self, Hash},
    difficulty, emission,
    params::{MAX_TRANSACTIONS, Network},
    privacy::{self, ShieldedTx, TxContext},
    state::{self, ChainState, MAX_FUTURE_SECONDS, ProofCache, Verification},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const SCHEMA_VERSION: &str = "1";
pub const MAX_MEMPOOL_BYTES: usize = 8_000_000;
pub const MAX_MEMPOOL_TRANSACTIONS: usize = 256;
const MAX_EXPIRY_DELTA: u64 = 720;

/// Local admission policy declined a well-formed object. Honest peers reach these
/// through ordinary timing and capacity races, so they are not misbehavior.
#[derive(Debug)]
pub struct PolicyRejection(pub &'static str);
impl std::fmt::Display for PolicyRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for PolicyRejection {}

/// The block's parent is not stored here; the sender may simply be ahead of us.
#[derive(Debug)]
pub struct UnknownParent;
impl std::fmt::Display for UnknownParent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unknown parent block")
    }
}
impl std::error::Error for UnknownParent {}

pub fn is_policy_rejection(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PolicyRejection>().is_some()
}
pub fn is_unknown_parent(error: &anyhow::Error) -> bool {
    error.downcast_ref::<UnknownParent>().is_some()
}
fn policy(reason: &'static str) -> anyhow::Error {
    PolicyRejection(reason).into()
}

// Miner policy only: higher public fee per encoded byte first. Cross products
// preserve exact ordering without floating-point monetary arithmetic.
fn fee_rate_order(a_fee: u64, a_bytes: usize, b_fee: u64, b_bytes: usize) -> std::cmp::Ordering {
    (u128::from(b_fee) * a_bytes as u128).cmp(&(u128::from(a_fee) * b_bytes as u128))
}
/// Lowest-fee-rate entries `(id, fee, bytes)` to drop so a new transaction fits.
/// `None` when it cannot fit by displacing only strictly lower fee rates.
fn eviction_plan(
    pool: &[(Hash, u64, usize)],
    max_transactions: usize,
    max_bytes: usize,
    fee: u64,
    bytes: usize,
) -> Option<Vec<Hash>> {
    let mut count = pool.len() + 1;
    let mut used = pool.iter().map(|entry| entry.2).sum::<usize>() + bytes;
    let fits = |count: usize, used: usize| count <= max_transactions && used <= max_bytes;
    let mut ordered = pool.to_vec();
    ordered.sort_by(|a, b| {
        fee_rate_order(a.1, a.2, b.1, b.2)
            .reverse()
            .then_with(|| b.0.cmp(&a.0))
    });
    let mut evicted = Vec::new();
    for (id, entry_fee, entry_bytes) in ordered {
        if fits(count, used) {
            break;
        }
        if fee_rate_order(fee, bytes, entry_fee, entry_bytes) != std::cmp::Ordering::Less {
            return None;
        }
        evicted.push(id);
        count -= 1;
        used -= entry_bytes;
    }
    fits(count, used).then_some(evicted)
}
pub struct Chain {
    pub state: ChainState,
    pub active: Vec<Block>,
    pub mempool: BTreeMap<Hash, ShieldedTx>,
    mempool_sizes: BTreeMap<Hash, usize>,
    mempool_used: usize,
    db: Connection,
    _lock: std::fs::File,
    pub cache: ProofCache,
    mempool_max_transactions: usize,
    mempool_max_bytes: usize,
}
#[derive(Debug)]
pub struct Acceptance {
    pub hash: Hash,
    pub became_best: bool,
    pub reorg_depth: usize,
    /// Cumulative work of the accepted block's branch; `None` for a block that
    /// was already stored. Local relay policy only, never a validity rule.
    pub branch_work: Option<u128>,
}
impl Chain {
    pub fn open(path: &Path, network: Network) -> Result<Self> {
        network.ensure_runnable()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .context("chain database already open in another node")?;
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let integrity: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        ensure!(integrity == "ok", "database integrity check failed");
        db.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS blocks(hash BLOB PRIMARY KEY,parent BLOB NOT NULL,height INTEGER NOT NULL,work TEXT NOT NULL,bytes BLOB NOT NULL); CREATE INDEX IF NOT EXISTS block_height ON blocks(height);")?;
        for (key, value) in [
            ("schema", SCHEMA_VERSION.to_owned()),
            ("network", network.chain_id()),
            ("genesis", hex::encode(network.genesis())),
        ] {
            let existing: Option<String> = db
                .query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
                .optional()?;
            if let Some(existing) = existing {
                ensure!(
                    existing == value,
                    "incompatible database {key}; never reuse another network or schema data directory"
                );
            } else {
                db.execute(
                    "INSERT INTO meta(key,value) VALUES(?,?)",
                    params![key, value],
                )?;
            }
        }
        let tip: Option<String> = db
            .query_row("SELECT value FROM meta WHERE key='tip'", [], |r| r.get(0))
            .optional()?;
        let mut chain = Self {
            state: ChainState::new(network),
            active: vec![],
            mempool: BTreeMap::new(),
            mempool_sizes: BTreeMap::new(),
            mempool_used: 0,
            db,
            _lock: lock,
            cache: ProofCache::default(),
            mempool_max_transactions: MAX_MEMPOOL_TRANSACTIONS,
            mempool_max_bytes: MAX_MEMPOOL_BYTES,
        };
        if let Some(tip) = tip {
            let hash: Hash = hex::decode(tip)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("corrupt tip"))?;
            chain.active = chain.ancestry(hash)?;
            let mut state = ChainState::new(network);
            for block in &chain.active {
                state
                    .apply_block_in_place(block, STORED_NOW, &mut chain.cache, Verification::Stored)
                    .context("database chain replay failed")?;
            }
            chain.state = state;
        }
        Ok(chain)
    }
    fn ancestry(&self, mut hash: Hash) -> Result<Vec<Block>> {
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        while hash != self.state.network.genesis() {
            ensure!(seen.insert(hash), "cyclic/corrupt ancestry");
            let block = self.stored(hash)?.context("unknown parent block")?;
            hash = block.header.previous;
            result.push(block);
        }
        result.reverse();
        Ok(result)
    }
    fn stored(&self, hash: Hash) -> Result<Option<Block>> {
        let bytes: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT bytes FROM blocks WHERE hash=?",
                [hash.as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let block = Block::decode(&bytes)?;
        ensure!(block.hash()? == hash, "stored hash mismatch");
        Ok(Some(block))
    }
    pub fn knows(&self, hash: Hash) -> Result<bool> {
        Ok(hash == self.state.network.genesis()
            || self
                .db
                .query_row(
                    "SELECT 1 FROM blocks WHERE hash=?",
                    [hash.as_slice()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
    }
    /// Zero-based position of `hash` in the active chain, found through the
    /// stored height index rather than by hashing the whole chain.
    pub fn active_position(&self, hash: Hash) -> Result<Option<usize>> {
        let height: Option<u64> = self
            .db
            .query_row(
                "SELECT height FROM blocks WHERE hash=?",
                [hash.as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(index) = height
            .and_then(|h| usize::try_from(h).ok())
            .and_then(|h| h.checked_sub(1))
        else {
            return Ok(None);
        };
        Ok(match self.active.get(index) {
            Some(block) if block.hash()? == hash => Some(index),
            _ => None,
        })
    }
    /// Stored ancestors of `parent` back to the active chain: the number of shared
    /// active blocks, then the side blocks above them, oldest first.
    fn side_branch(&self, parent: Hash) -> Result<(usize, Vec<Block>)> {
        let mut side = Vec::new();
        let mut hash = parent;
        let mut last_height = u64::MAX;
        let fork = loop {
            if hash == self.state.network.genesis() {
                break 0;
            }
            if let Some(index) = self.active_position(hash)? {
                break index + 1;
            }
            let Some(block) = self.stored(hash)? else {
                ensure!(side.is_empty(), "corrupt side-branch ancestry");
                return Err(UnknownParent.into());
            };
            ensure!(
                block.header.height < last_height,
                "corrupt side-branch ancestry"
            );
            last_height = block.header.height;
            hash = block.header.previous;
            side.push(block);
        };
        side.reverse();
        Ok((fork, side))
    }
    fn store(&mut self, hash: Hash, block: &Block, work: u128, tip: bool) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO blocks(hash,parent,height,work,bytes) VALUES(?,?,?,?,?)",
            params![
                hash.as_slice(),
                block.header.previous.as_slice(),
                block.header.height,
                work.to_string(),
                block.bytes()?
            ],
        )?;
        if tip {
            tx.execute("INSERT INTO meta(key,value) VALUES('tip',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[hex::encode(hash)])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn accept(&mut self, block: Block) -> Result<Acceptance> {
        block.check_shape()?;
        ensure!(
            block.header.network == self.state.network,
            "block network mismatch"
        );
        let hash = block.hash()?;
        if self.knows(hash)? {
            return Ok(Acceptance {
                hash,
                became_best: false,
                reorg_depth: 0,
                branch_work: None,
            });
        }
        let now = unix_time();
        if block.header.timestamp > now.saturating_add(MAX_FUTURE_SECONDS) {
            return Err(policy("block timestamp is too far in the future"));
        }
        if block.header.previous == self.state.best_hash {
            let mut next = self.state.clone();
            next.apply_block_in_place(&block, now, &mut self.cache, Verification::Full)?;
            self.store(hash, &block, next.work, true)?;
            let branch_work = Some(next.work);
            self.state = next;
            self.active.push(block);
            self.readmit(Vec::new());
            return Ok(Acceptance {
                hash,
                became_best: true,
                reorg_depth: 0,
                branch_work,
            });
        }
        let (fork, side) = self.side_branch(block.header.previous)?;
        // Every header rule and the block's own RandomX work are checked against
        // the branch headers before any stored block is replayed, so forcing a
        // branch replay always costs real work at the branch's difficulty.
        {
            let network = self.state.network;
            let ancestors = fork + side.len();
            ensure!(
                block.header.height == ancestors as u64 + 1,
                "wrong chain/height/parent"
            );
            let header_at = |i: usize| -> &Header {
                if i < fork {
                    &self.state.headers[i]
                } else {
                    &side[i - fork].header
                }
            };
            let tail: Vec<Header> = (ancestors.saturating_sub(difficulty::WINDOW + 1)..ancestors)
                .map(|i| header_at(i).clone())
                .collect();
            let seed_ancestor = state::pow_seed_index(block.header.height)
                .map(|i| {
                    (i < ancestors)
                        .then(|| header_at(i))
                        .context("missing RandomX seed ancestor")
                })
                .transpose()?;
            state::check_header_context(network, &tail, seed_ancestor, &block.header, now)?;
            crate::pow::verify(
                block.header.pow_seed,
                &block.header.bytes()?,
                block.header.difficulty,
            )?;
        }
        let mut next = ChainState::new(self.state.network);
        for stored in self.active[..fork].iter().chain(&side) {
            next.apply_block_in_place(stored, STORED_NOW, &mut self.cache, Verification::Stored)
                .context("stored branch replay failed")?;
        }
        next.apply_block_in_place(&block, now, &mut self.cache, Verification::Full)?;
        let became_best = next.work > self.state.work;
        let branch_work = Some(next.work);
        self.store(hash, &block, next.work, became_best)?;
        if !became_best {
            return Ok(Acceptance {
                hash,
                became_best: false,
                reorg_depth: 0,
                branch_work,
            });
        }
        let replaced = self.active.split_off(fork);
        let reorg_depth = replaced.len();
        self.active.extend(side);
        self.active.push(block);
        self.state = next;
        self.readmit(replaced);
        Ok(Acceptance {
            hash,
            became_best: true,
            reorg_depth,
            branch_work,
        })
    }
    /// Re-evaluates pending and disconnected transactions against the new tip.
    fn readmit(&mut self, replaced: Vec<Block>) {
        let mut candidates: Vec<ShieldedTx> =
            std::mem::take(&mut self.mempool).into_values().collect();
        self.mempool_sizes.clear();
        self.mempool_used = 0;
        for old in replaced {
            candidates.extend(old.body.transactions);
        }
        for candidate in candidates {
            let _ = self.submit(candidate);
        }
    }
    fn remove_from_mempool(&mut self, id: &Hash) {
        self.mempool.remove(id);
        if let Some(bytes) = self.mempool_sizes.remove(id) {
            self.mempool_used -= bytes;
        }
    }
    pub fn submit(&mut self, tx: ShieldedTx) -> Result<Hash> {
        let id = tx.id()?;
        if self.mempool.contains_key(&id) {
            return Err(policy("duplicate mempool transaction"));
        }
        let bytes = codec::encode(&tx)?.len();
        ensure!(bytes <= codec::MAX_TX_BYTES, "transaction too large");
        let height = self.state.height + 1;
        let expires = tx.context.expires;
        if expires < height || expires > height.saturating_add(MAX_EXPIRY_DELTA) {
            return Err(policy("transaction expired or expiry too far ahead"));
        }
        if !self.state.anchors.contains_key(&tx.bundle.anchor) {
            return Err(policy("unknown or expired commitment anchor"));
        }
        if tx
            .bundle
            .actions
            .iter()
            .any(|a| self.state.nullifiers.contains(&a.nullifier))
        {
            return Err(policy("nullifier already spent"));
        }
        let claimed: BTreeSet<_> = self
            .mempool
            .values()
            .flat_map(|t| t.bundle.actions.iter().map(|a| a.nullifier))
            .collect();
        if tx
            .bundle
            .actions
            .iter()
            .any(|a| claimed.contains(&a.nullifier))
        {
            return Err(policy("mempool nullifier conflict"));
        }
        let pool: Vec<_> = self
            .mempool
            .iter()
            .map(|(id, t)| {
                (
                    *id,
                    t.bundle.value_balance.max(0) as u64,
                    self.mempool_sizes
                        .get(id)
                        .copied()
                        .unwrap_or(codec::MAX_TX_BYTES),
                )
            })
            .collect();
        let evict = eviction_plan(
            &pool,
            self.mempool_max_transactions,
            self.mempool_max_bytes,
            tx.bundle.value_balance.max(0) as u64,
            bytes,
        )
        .ok_or_else(|| policy("mempool full"))?;
        self.state.validate_transfer(&tx, height, &mut self.cache)?;
        for old in &evict {
            self.remove_from_mempool(old);
        }
        self.mempool.insert(id, tx);
        self.mempool_sizes.insert(id, bytes);
        self.mempool_used += bytes;
        Ok(id)
    }
    pub fn mempool_bytes(&self) -> usize {
        self.mempool_used
    }
    pub fn configure_mempool(&mut self, transactions: usize, bytes: usize) -> Result<()> {
        ensure!(
            self.mempool.is_empty(),
            "configure mempool before admitting transactions"
        );
        ensure!(
            (1..=MAX_MEMPOOL_TRANSACTIONS).contains(&transactions),
            "invalid mempool transaction limit"
        );
        ensure!(
            (codec::MAX_TX_BYTES..=MAX_MEMPOOL_BYTES).contains(&bytes),
            "invalid mempool byte limit"
        );
        self.mempool_max_transactions = transactions;
        self.mempool_max_bytes = bytes;
        Ok(())
    }
    pub fn template(&mut self, recipient: orchard::Address) -> Result<Block> {
        self.template_plan()?.build(recipient)
    }
    /// Selects template transactions against the current tip. Proving happens in
    /// `TemplatePlan::build`, which needs no access to the chain.
    pub fn template_plan(&mut self) -> Result<TemplatePlan> {
        let height = self.state.height + 1;
        let state = self.state.clone();
        let mut txs = Vec::new();
        let mut reserved = BTreeSet::new();
        let mut selected_bytes = 0usize;
        let mut selected_fees = 0u128;
        let mut candidates = self
            .mempool
            .iter()
            .map(|(id, tx)| {
                (
                    id,
                    tx,
                    self.mempool_sizes
                        .get(id)
                        .copied()
                        .unwrap_or(codec::MAX_TX_BYTES),
                )
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|(a_id, a, a_bytes), (b_id, b, b_bytes)| {
            fee_rate_order(
                a.bundle.value_balance.max(0) as u64,
                *a_bytes,
                b.bundle.value_balance.max(0) as u64,
                *b_bytes,
            )
            .then_with(|| a_id.cmp(b_id))
        });
        for (_, tx, tx_bytes) in candidates {
            if txs.len() >= MAX_TRANSACTIONS {
                break;
            }
            if selected_bytes + tx_bytes > codec::MAX_BLOCK_BYTES - 100_000 {
                continue;
            }
            let fee = u128::try_from(tx.bundle.value_balance).unwrap_or(u128::MAX);
            if fee
                > (i64::MAX as u128)
                    .saturating_sub(emission::reward(height).miner())
                    .saturating_sub(selected_fees)
            {
                continue;
            }
            if state.validate_transfer(tx, height, &mut self.cache).is_ok()
                && tx
                    .bundle
                    .actions
                    .iter()
                    .all(|a| !reserved.contains(&a.nullifier))
            {
                for a in &tx.bundle.actions {
                    reserved.insert(a.nullifier);
                }
                txs.push(tx.clone());
                selected_bytes += tx_bytes;
                selected_fees += fee;
            }
        }
        Ok(TemplatePlan {
            timestamp_floor: state.median_time() + 1,
            difficulty: state.next_difficulty(),
            pow_seed: state.next_pow_seed(),
            state,
            cache: self.cache.clone(),
            transactions: txs,
        })
    }
}

/// Template inputs captured under the node lock; `build` performs the slow
/// coinbase proving without holding it.
pub struct TemplatePlan {
    state: ChainState,
    cache: ProofCache,
    transactions: Vec<ShieldedTx>,
    timestamp_floor: u64,
    difficulty: u64,
    pow_seed: Hash,
}
impl TemplatePlan {
    pub fn build(self, recipient: orchard::Address) -> Result<Block> {
        let Self {
            mut state,
            mut cache,
            transactions: txs,
            timestamp_floor,
            difficulty,
            pow_seed,
        } = self;
        let height = state.height + 1;
        let previous = state.best_hash;
        let network = state.network;
        let fees = txs
            .iter()
            .map(|t| t.bundle.value_balance as u128)
            .sum::<u128>();
        let reward = emission::reward(height);
        let context = |kind| TxContext {
            protocol: 1,
            network,
            kind,
            height,
            expires: 0,
            nonce: previous,
        };
        let miner = privacy::mint(
            context(1),
            recipient,
            (reward.miner() + fees).try_into()?,
            false,
        )?;
        let treasury = if reward.treasury > 0 {
            Some(privacy::mint(
                context(2),
                privacy::treasury_address(network),
                reward.treasury.try_into()?,
                true,
            )?)
        } else {
            None
        };
        let body = Body {
            miner,
            treasury,
            transactions: txs,
        };
        state.apply_body(height, previous, &body, &mut cache)?;
        let header = Header {
            version: 1,
            network,
            height,
            previous,
            timestamp: unix_time().max(timestamp_floor),
            difficulty,
            pow_seed,
            nonce: 0,
            body_hash: body.hash()?,
            notes_root: state.notes_root(),
            nullifiers_root: state.nullifiers_root(),
        };
        Ok(Block { header, body })
    }
}

/// Replays of stored blocks: the future-time bound was enforced at admission and
/// must not fail later because the local clock moved backwards.
const STORED_NOW: u64 = u64::MAX;

pub fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod fee_policy_tests {
    use super::{eviction_plan, fee_rate_order};

    #[test]
    fn exact_fee_rate_order_handles_different_sizes_and_large_values() {
        use std::cmp::Ordering;
        assert_eq!(fee_rate_order(10, 100, 9, 100), Ordering::Less);
        assert_eq!(fee_rate_order(10, 100, 15, 200), Ordering::Less);
        assert_eq!(fee_rate_order(10, 100, 20, 200), Ordering::Equal);
        assert_eq!(
            fee_rate_order(u64::MAX, 200_000, u64::MAX - 1, 200_000),
            Ordering::Less
        );
        let mut samples = [(10, 100), (5, 100), (30, 200), (20, 100)];
        samples.sort_by(|a, b| fee_rate_order(a.0, a.1, b.0, b.1));
        assert_eq!(samples, [(20, 100), (30, 200), (10, 100), (5, 100)]);
    }

    #[test]
    fn a_full_pool_admits_only_strictly_better_fee_rates() {
        let pool = [([1; 32], 10, 100), ([2; 32], 50, 100), ([3; 32], 20, 100)];
        assert_eq!(eviction_plan(&pool, 4, 1_000, 1, 100), Some(vec![]));
        assert_eq!(eviction_plan(&pool, 3, 1_000, 11, 100), Some(vec![[1; 32]]));
        assert_eq!(eviction_plan(&pool, 3, 1_000, 10, 100), None);
        assert_eq!(eviction_plan(&pool, 3, 1_000, 5, 100), None);
        // Room by bytes: two low-rate entries leave for one larger, richer one.
        assert_eq!(
            eviction_plan(&pool, 10, 300, 100, 200),
            Some(vec![[1; 32], [3; 32]])
        );
        // The second displaced entry would need a strictly lower rate.
        assert_eq!(eviction_plan(&pool, 10, 300, 30, 200), None);
        // Equal-rate entries are displaced in a deterministic order.
        let tied = [([7; 32], 10, 100), ([8; 32], 10, 100)];
        assert_eq!(eviction_plan(&tied, 2, 1_000, 11, 100), Some(vec![[8; 32]]));
    }
}
