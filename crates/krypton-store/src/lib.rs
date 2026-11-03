//! Persistent chain state (redb) with fork choice by cumulative work.
//!
//! Every block is fully validated before it joins the main chain; side branches
//! are stored after a header and proof-of-work check and adopted only when they
//! carry more work. Undo records make reorganizations exact.

#![forbid(unsafe_code)]

use anyhow::{Context, Result, anyhow, bail};
use krypton_core::codec::{Reader, Writer};
use krypton_core::consensus::{self, ParentContext, StateView};
use krypton_core::mmr::{self, MmrNode, MmrProof, MmrStore};
use krypton_core::reward::{Account, Payout, RewardView};
use krypton_core::tree::{self, Frontier, NodeStore};
use krypton_core::types::{Block, BlockHeader};
use krypton_core::{ChainParams, CoreError, Hash32, Network};
use krypton_crypto::field::{Digest, digest_from_bytes, digest_to_bytes};
use krypton_pow::Verifier;
use parking_lot::{Mutex, RwLock};
use redb::{Database, ReadOnlyTable, ReadableTable, TableDefinition, WriteTransaction};
use std::path::Path;
use std::sync::Arc;

const HEADERS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("headers");
const BLOCKS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("blocks");
const MAIN: TableDefinition<u64, &[u8]> = TableDefinition::new("main");
const NULLIFIERS: TableDefinition<&[u8], u64> = TableDefinition::new("nullifiers");
const TREE: TableDefinition<u128, &[u8]> = TableDefinition::new("tree");
const ROOTS: TableDefinition<&[u8], u64> = TableDefinition::new("roots");
const MMR: TableDefinition<u64, &[u8]> = TableDefinition::new("mmr");
const UNDO: TableDefinition<&[u8], &[u8]> = TableDefinition::new("undo");
const LEAFSTART: TableDefinition<u64, u64> = TableDefinition::new("leafstart");
const FRONTIERS: TableDefinition<u64, &[u8]> = TableDefinition::new("frontiers");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const ADDRESSES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("addresses");
const TXINDEX: TableDefinition<&[u8], u64> = TableDefinition::new("txindex");
/// Share hash -> height of the main-chain block that credited it.
const SHARES: TableDefinition<&[u8], u64> = TableDefinition::new("shares");
/// Miner owner -> unpaid reward account.
const ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
/// (since, big-endian) ++ owner -> 0: accounts ordered by payout due time.
const DUE: TableDefinition<&[u8], u8> = TableDefinition::new("due");
/// Height -> payouts made by that main-chain block.
const PAYOUTS: TableDefinition<u64, &[u8]> = TableDefinition::new("payouts");

fn due_key(since: u64, owner: &Hash32) -> [u8; 40] {
    let mut k = [0u8; 40];
    k[..8].copy_from_slice(&since.to_be_bytes());
    k[8..].copy_from_slice(owner);
    k
}

fn encode_payouts(v: &[Payout]) -> Vec<u8> {
    let mut w = Writer::default();
    w.u32(v.len() as u32);
    for p in v {
        w.raw(&p.owner).u64(p.value);
    }
    w.0
}

fn decode_payouts(b: &[u8]) -> Result<Vec<Payout>> {
    let mut r = Reader::new(b);
    let n = r.u32()? as usize;
    let mut v = Vec::with_capacity(n.min(krypton_core::params::MAX_PAYOUTS_PER_BLOCK));
    for _ in 0..n {
        v.push(Payout {
            owner: r.array()?,
            value: r.u64()?,
        });
    }
    r.finish()?;
    Ok(v)
}

pub const FRONTIER_CHECKPOINT: u64 = 64;
const HISTORY: usize = krypton_core::difficulty::WINDOW + 1;

const STATUS_VALID: u8 = 1;
const STATUS_INVALID: u8 = 2;
const STATUS_UNVERIFIED: u8 = 3;

#[derive(Clone, Debug)]
pub struct HeaderRecord {
    pub header: BlockHeader,
    pub work: u128,
    pub status: u8,
}

impl HeaderRecord {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(self.header.to_bytes());
        w.u128(self.work).u8(self.status);
        w.0
    }
    fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut r = Reader::new(b);
        let header = BlockHeader::from_reader(&mut r)?;
        let work = r.u128()?;
        let status = r.u8()?;
        Ok(Self {
            header,
            work,
            status,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Tip {
    pub header: BlockHeader,
    pub hash: Hash32,
    pub work: u128,
    pub frontier: Frontier,
}

impl Tip {
    pub fn height(&self) -> u64 {
        self.header.height
    }
    /// Blocks in the MMR (genesis included).
    pub fn mmr_leaves(&self) -> u64 {
        self.header.height + 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Connected { reorg_depth: u64 },
    SideChain,
    AlreadyKnown,
    Orphan { missing: Hash32 },
}

#[derive(Debug, thiserror::Error)]
pub enum ChainError {
    #[error("invalid block: {0}")]
    Invalid(#[from] CoreError),
    #[error("storage: {0}")]
    Storage(#[from] anyhow::Error),
}

/// Read-only snapshot of the state used for validation.
pub struct View {
    nullifiers: ReadOnlyTable<&'static [u8], u64>,
    roots: ReadOnlyTable<&'static [u8], u64>,
    tree: ReadOnlyTable<u128, &'static [u8]>,
    mmr: ReadOnlyTable<u64, &'static [u8]>,
    main: ReadOnlyTable<u64, &'static [u8]>,
    shares: ReadOnlyTable<&'static [u8], u64>,
    accounts: ReadOnlyTable<&'static [u8], &'static [u8]>,
    due: ReadOnlyTable<&'static [u8], u8>,
    pool: u64,
    tip_height: u64,
}

fn tree_key(level: u8, index: u64) -> u128 {
    ((level as u128) << 64) | index as u128
}

impl StateView for View {
    fn nullifier_spent(&self, nf: &Hash32) -> bool {
        self.nullifiers.get(nf.as_slice()).ok().flatten().is_some()
    }
    fn anchor_height(&self, root: &Hash32) -> Option<u64> {
        let h = self.roots.get(root.as_slice()).ok().flatten()?.value();
        // Only roots of the current main chain count.
        (h <= self.tip_height).then_some(h)
    }
    fn main_hash(&self, height: u64) -> Option<Hash32> {
        if height > self.tip_height {
            return None;
        }
        View::main_hash(self, height)
    }
    fn share_seen(&self, hash: &Hash32) -> bool {
        self.shares.get(hash.as_slice()).ok().flatten().is_some()
    }
}

impl RewardView for View {
    fn pool(&self) -> u64 {
        self.pool
    }
    fn account(&self, owner: &Hash32) -> Option<Account> {
        Account::from_bytes(self.accounts.get(owner.as_slice()).ok().flatten()?.value())
    }
    fn due_accounts(&self, max_since: u64, limit: usize) -> Vec<(Hash32, Account)> {
        let hi = due_key(max_since, &[0xff; 32]);
        let Ok(range) = self.due.range::<&[u8]>(..=hi.as_slice()) else {
            return vec![];
        };
        range
            .filter_map(|e| e.ok())
            .filter_map(|(k, _)| {
                let owner: Hash32 = k.value().get(8..40)?.try_into().ok()?;
                Some((owner, self.account(&owner)?))
            })
            .take(limit)
            .collect()
    }
}

impl NodeStore for View {
    fn get(&self, level: u8, index: u64) -> Option<Digest> {
        let v = self.tree.get(tree_key(level, index)).ok().flatten()?;
        digest_from_bytes(v.value().try_into().ok()?).ok()
    }
}

impl MmrStore for View {
    fn mmr_get(&self, pos: u64) -> Option<MmrNode> {
        MmrNode::from_bytes(self.mmr.get(pos).ok().flatten()?.value())
    }
}

impl View {
    pub fn main_hash(&self, height: u64) -> Option<Hash32> {
        self.main
            .get(height)
            .ok()
            .flatten()?
            .value()
            .try_into()
            .ok()
    }
}

/// Write-side MMR reader combining committed nodes with pending ones.
struct PendingMmr<'a> {
    view: &'a View,
    pending: Vec<(u64, MmrNode)>,
}
impl MmrStore for PendingMmr<'_> {
    fn mmr_get(&self, pos: u64) -> Option<MmrNode> {
        self.pending
            .iter()
            .rev()
            .find(|(p, _)| *p == pos)
            .map(|x| x.1)
            .or_else(|| self.view.mmr_get(pos))
    }
}

pub struct Chain {
    pub params: ChainParams,
    pub pow: Arc<Verifier>,
    pub genesis: Block,
    db: Database,
    write: Mutex<()>,
    tip: RwLock<Tip>,
    disconnected: Mutex<Vec<krypton_core::Transaction>>,
}

const MAX_DISCONNECTED_TXS: usize = 4096;
/// Set once the 1.0.2 repair has cleared invalid markers that earlier versions could set
/// wrongly.
const META_INVALID_RESET: &str = "repair/invalid-reset-v1";

/// A chain database from another genesis (an earlier launch of the same network) holds
/// nothing this chain can use; it is moved aside, never deleted, and sync starts fresh.
fn retire_foreign_chain(dir: &Path, genesis: &Hash32) -> Result<()> {
    let path = dir.join("chain.redb");
    if !path.exists() {
        return Ok(());
    }
    let stored = {
        let db = Database::create(&path).context("open chain database")?;
        let r = db.begin_read()?;
        match r.open_table(META) {
            Ok(t) => t.get("genesis")?.map(|g| g.value().to_vec()),
            Err(_) => None,
        }
    };
    let Some(stored) = stored else {
        return Ok(());
    };
    if stored.as_slice() == genesis.as_slice() {
        return Ok(());
    }
    let id: String = stored.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let aside = dir.join(format!("chain-{id}.redb.retired"));
    std::fs::rename(&path, &aside).context("move the old chain database aside")?;
    tracing::warn!(
        "chain database belonged to genesis {id}…; moved to {} and starting fresh",
        aside.display()
    );
    Ok(())
}

fn h32(v: &[u8]) -> Result<Hash32> {
    v.try_into().map_err(|_| anyhow!("bad hash length"))
}

impl Chain {
    pub fn open(dir: &Path, network: Network) -> Result<Self> {
        let params = network.params();
        if !params.is_launched() {
            bail!(
                "{} has not launched yet: its genesis anchor is set by the launch release",
                params.name
            );
        }
        std::fs::create_dir_all(dir)?;
        let genesis = consensus::genesis_block(&params);
        retire_foreign_chain(dir, &genesis.hash())?;
        let db = Database::create(dir.join("chain.redb")).context("open chain database")?;
        let pow = Arc::new(Verifier::new(params.pow, genesis.hash()));
        let w = db.begin_write()?;
        {
            for t in [HEADERS, BLOCKS, UNDO, ADDRESSES] {
                w.open_table(t)?;
            }
            w.open_table(NULLIFIERS)?;
            w.open_table(ROOTS)?;
            w.open_table(TREE)?;
            w.open_table(MAIN)?;
            w.open_table(MMR)?;
            w.open_table(FRONTIERS)?;
            w.open_table(LEAFSTART)?;
            w.open_table(TXINDEX)?;
            w.open_table(SHARES)?;
            w.open_table(ACCOUNTS)?;
            w.open_table(DUE)?;
            w.open_table(PAYOUTS)?;
            let meta = w.open_table(META)?;
            if let Some(g) = meta.get("genesis")?
                && g.value() != genesis.hash()
            {
                bail!("database belongs to a different chain");
            }
        }
        w.commit()?;
        let chain = Self {
            tip: RwLock::new(Tip {
                header: genesis.header.clone(),
                hash: genesis.hash(),
                work: 0,
                frontier: Frontier::new(),
            }),
            params,
            pow,
            genesis,
            db,
            write: Mutex::new(()),
            disconnected: Mutex::new(Vec::new()),
        };
        chain.init()?;
        chain.repair()?;
        Ok(chain)
    }

    /// Repairs state that kryptond 1.0.0 and 1.0.1 could leave inconsistent; cheap and
    /// idempotent.
    ///
    /// * Anchor roots of the last `ANCHOR_WINDOW` blocks are rewritten from the main
    ///   chain: undoing a block that added no note leaves deleted a root that was still
    ///   valid, so the node rejected transfers (and blocks) every other node accepts.
    /// * Once: headers marked invalid are made unverified again. A relayed block with an
    ///   altered body, or the missing root above, could get a valid block marked invalid;
    ///   bodies are fetched and fully validated again when peers offer them.
    fn repair(&self) -> Result<()> {
        let tip = self.tip().height();
        let w = self.db.begin_write()?;
        let (roots_fixed, reset) = {
            let main = w.open_table(MAIN)?;
            let mut headers = w.open_table(HEADERS)?;
            let mut roots = w.open_table(ROOTS)?;
            let mut meta = w.open_table(META)?;
            // Highest main-chain height of every root in the window.
            let mut want: std::collections::BTreeMap<Hash32, u64> = Default::default();
            let from = tip.saturating_sub(krypton_core::params::ANCHOR_WINDOW + 1);
            for h in from..=tip {
                let Some(hash) = main.get(h)?.map(|v| v.value().to_vec()) else {
                    continue;
                };
                let Some(rec) = headers.get(hash.as_slice())?.map(|v| v.value().to_vec()) else {
                    continue;
                };
                want.insert(HeaderRecord::from_bytes(&rec)?.header.note_root, h);
            }
            let mut roots_fixed = 0usize;
            for (root, h) in want {
                if roots.get(root.as_slice())?.map(|v| v.value()) != Some(h) {
                    roots.insert(root.as_slice(), h)?;
                    roots_fixed += 1;
                }
            }
            let mut reset = 0usize;
            if meta.get(META_INVALID_RESET)?.is_none() {
                let mut invalid = Vec::new();
                for e in headers.iter()? {
                    let (k, v) = e?;
                    let rec = HeaderRecord::from_bytes(v.value())?;
                    if rec.status == STATUS_INVALID {
                        invalid.push((k.value().to_vec(), rec));
                    }
                }
                for (k, mut rec) in invalid {
                    rec.status = STATUS_UNVERIFIED;
                    headers.insert(k.as_slice(), rec.to_bytes().as_slice())?;
                    reset += 1;
                }
                meta.insert(META_INVALID_RESET, [1u8].as_slice())?;
            }
            (roots_fixed, reset)
        };
        w.commit()?;
        if roots_fixed > 0 || reset > 0 {
            tracing::warn!(
                roots_fixed,
                invalid_reset = reset,
                "repaired chain state written by an earlier version"
            );
        }
        Ok(())
    }

    fn init(&self) -> Result<()> {
        let r = self.db.begin_read()?;
        let meta = r.open_table(META)?;
        if let Some(t) = meta.get("tip")? {
            let hash = h32(t.value())?;
            let rec = self.header_record(&hash)?.context("tip header missing")?;
            let frontier =
                Frontier::from_bytes(meta.get("frontier")?.context("frontier missing")?.value())
                    .context("corrupt frontier")?;
            *self.tip.write() = Tip {
                header: rec.header,
                hash,
                work: rec.work,
                frontier,
            };
            return Ok(());
        }
        drop(meta);
        drop(r);
        let g = &self.genesis;
        let hash = g.hash();
        let w = self.db.begin_write()?;
        {
            let rec = HeaderRecord {
                header: g.header.clone(),
                work: g.header.difficulty,
                status: STATUS_VALID,
            };
            w.open_table(HEADERS)?
                .insert(hash.as_slice(), rec.to_bytes().as_slice())?;
            w.open_table(BLOCKS)?
                .insert(hash.as_slice(), g.to_bytes().as_slice())?;
            w.open_table(MAIN)?.insert(0, hash.as_slice())?;
            w.open_table(ROOTS)?
                .insert(g.header.note_root.as_slice(), 0)?;
            w.open_table(LEAFSTART)?.insert(0, 0)?;
            let leaf = MmrNode::leaf(&hash, g.header.difficulty);
            w.open_table(MMR)?.insert(0, leaf.to_bytes().as_slice())?;
            let f = Frontier::new();
            w.open_table(FRONTIERS)?
                .insert(0, f.to_bytes().as_slice())?;
            let mut meta = w.open_table(META)?;
            meta.insert("genesis", hash.as_slice())?;
            meta.insert("tip", hash.as_slice())?;
            meta.insert("frontier", f.to_bytes().as_slice())?;
        }
        w.commit()?;
        *self.tip.write() = Tip {
            header: g.header.clone(),
            hash,
            work: g.header.difficulty,
            frontier: Frontier::new(),
        };
        Ok(())
    }

    pub fn tip(&self) -> Tip {
        self.tip.read().clone()
    }

    pub fn view(&self) -> Result<View> {
        let r = self.db.begin_read()?;
        let pool = match r.open_table(META)?.get("pool")? {
            Some(v) => {
                u64::from_le_bytes(v.value().try_into().map_err(|_| anyhow!("corrupt pool"))?)
            }
            None => 0,
        };
        Ok(View {
            nullifiers: r.open_table(NULLIFIERS)?,
            roots: r.open_table(ROOTS)?,
            tree: r.open_table(TREE)?,
            mmr: r.open_table(MMR)?,
            main: r.open_table(MAIN)?,
            shares: r.open_table(SHARES)?,
            accounts: r.open_table(ACCOUNTS)?,
            due: r.open_table(DUE)?,
            pool,
            tip_height: self.tip.read().height(),
        })
    }

    pub fn header_record(&self, hash: &Hash32) -> Result<Option<HeaderRecord>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(HEADERS)?;
        Ok(match t.get(hash.as_slice())? {
            Some(v) => Some(HeaderRecord::from_bytes(v.value())?),
            None => None,
        })
    }

    pub fn block(&self, hash: &Hash32) -> Result<Option<Block>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(BLOCKS)?;
        Ok(match t.get(hash.as_slice())? {
            Some(v) => Some(Block::from_bytes(v.value())?),
            None => None,
        })
    }

    pub fn has_block(&self, hash: &Hash32) -> Result<bool> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(BLOCKS)?.get(hash.as_slice())?.is_some())
    }

    pub fn main_hash(&self, height: u64) -> Result<Option<Hash32>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(MAIN)?;
        Ok(match t.get(height)? {
            Some(v) => Some(h32(v.value())?),
            None => None,
        })
    }

    pub fn header_at(&self, height: u64) -> Result<Option<BlockHeader>> {
        match self.main_hash(height)? {
            Some(h) => Ok(self.header_record(&h)?.map(|r| r.header)),
            None => Ok(None),
        }
    }

    pub fn block_at(&self, height: u64) -> Result<Option<Block>> {
        match self.main_hash(height)? {
            Some(h) => self.block(&h),
            None => Ok(None),
        }
    }

    pub fn is_main(&self, hash: &Hash32) -> Result<bool> {
        let Some(rec) = self.header_record(hash)? else {
            return Ok(false);
        };
        Ok(self.main_hash(rec.header.height)? == Some(*hash))
    }

    /// (timestamp, difficulty) of up to HISTORY headers ending at `hash`, following parents.
    fn branch_history(&self, hash: &Hash32) -> Result<Vec<(u64, u128)>> {
        let mut out = Vec::with_capacity(HISTORY);
        let mut cur = *hash;
        while out.len() < HISTORY {
            let rec = self
                .header_record(&cur)?
                .context("missing ancestor header")?;
            out.push((rec.header.timestamp, rec.header.difficulty));
            if rec.header.height == 0 {
                break;
            }
            cur = rec.header.prev;
        }
        out.reverse();
        Ok(out)
    }

    /// Block locator for sync: recent hashes, then exponentially sparser.
    pub fn locator(&self) -> Result<Vec<Hash32>> {
        let tip = self.tip().height();
        let mut v = Vec::new();
        let mut step = 1u64;
        let mut h = tip as i64;
        while h >= 0 {
            if let Some(x) = self.main_hash(h as u64)? {
                v.push(x);
            }
            if v.len() >= 10 {
                step *= 2;
            }
            h -= step as i64;
        }
        if v.last() != Some(&self.genesis.hash()) {
            v.push(self.genesis.hash());
        }
        Ok(v)
    }

    /// Main-chain hashes after the first locator entry we know (for peers syncing from us).
    pub fn hashes_after(&self, locator: &[Hash32], max: usize) -> Result<Vec<Hash32>> {
        let mut start = 0;
        for h in locator {
            if self.is_main(h)? {
                start = self
                    .header_record(h)?
                    .map(|r| r.header.height + 1)
                    .unwrap_or(0);
                break;
            }
        }
        let tip = self.tip().height();
        let mut v = Vec::new();
        let mut h = start;
        while h <= tip && v.len() < max {
            if let Some(x) = self.main_hash(h)? {
                v.push(x);
            }
            h += 1;
        }
        Ok(v)
    }

    fn put_header(&self, w: &WriteTransaction, hash: &Hash32, rec: &HeaderRecord) -> Result<()> {
        w.open_table(HEADERS)?
            .insert(hash.as_slice(), rec.to_bytes().as_slice())?;
        Ok(())
    }

    /// Main entry point: validates and stores a block, performing fork choice.
    pub fn process_block(
        &self,
        block: &Block,
        now: u64,
    ) -> std::result::Result<Outcome, ChainError> {
        let _g = self.write.lock();
        let hash = block.hash();
        if let Some(rec) = self.header_record(&hash)? {
            if rec.status == STATUS_INVALID {
                return Err(ChainError::Invalid(CoreError::Invalid(
                    "block previously found invalid",
                )));
            }
            if self.has_block(&hash)? {
                return Ok(Outcome::AlreadyKnown);
            }
        }
        let Some(parent) = self.header_record(&block.header.prev)? else {
            return Ok(Outcome::Orphan {
                missing: block.header.prev,
            });
        };
        if parent.status == STATUS_INVALID {
            return Err(ChainError::Invalid(CoreError::Invalid("parent is invalid")));
        }
        // Cheap checks first: header context and proof of work on its own branch.
        let history = self.branch_history(&block.header.prev)?;
        let ctx = ParentContext {
            parent: &parent.header,
            history: &history,
            frontier: &Frontier::new(),
            mmr_root: block.header.mmr_root,
            matured: vec![],
        };
        consensus::check_header(&self.params, &self.pow, &ctx, &block.header, now)?;
        // Anyone can relay a real header with an altered body. Such a block is refused
        // before anything is stored, so the header is never remembered as invalid and the
        // genuine block is still accepted when it arrives.
        consensus::check_body_commitments(block)?;
        let work = parent.work.saturating_add(block.header.difficulty);
        let w = self.db.begin_write().map_err(anyhow::Error::from)?;
        self.put_header(
            &w,
            &hash,
            &HeaderRecord {
                header: block.header.clone(),
                work,
                status: STATUS_UNVERIFIED,
            },
        )?;
        w.open_table(BLOCKS)
            .map_err(anyhow::Error::from)?
            .insert(hash.as_slice(), block.to_bytes().as_slice())
            .map_err(anyhow::Error::from)?;
        w.commit().map_err(anyhow::Error::from)?;

        let tip = self.tip();
        if block.header.prev == tip.hash {
            match self.connect(block, now) {
                Ok(()) => Ok(Outcome::Connected { reorg_depth: 0 }),
                Err(e) => {
                    if matches!(e, ChainError::Invalid(_)) {
                        self.mark_invalid(&hash)?;
                    }
                    Err(e)
                }
            }
        } else if work > tip.work {
            self.reorganize(hash, now)
        } else {
            Ok(Outcome::SideChain)
        }
    }

    fn mark_invalid(&self, hash: &Hash32) -> Result<()> {
        if let Some(mut rec) = self.header_record(hash)? {
            rec.status = STATUS_INVALID;
            let w = self.db.begin_write()?;
            self.put_header(&w, hash, &rec)?;
            w.open_table(BLOCKS)?.remove(hash.as_slice())?;
            w.commit()?;
        }
        Ok(())
    }

    /// Payouts made by the main-chain block at `height`.
    pub fn payouts_at(&self, height: u64) -> Result<Vec<Payout>> {
        let r = self.db.begin_read()?;
        match r.open_table(PAYOUTS)?.get(height)? {
            Some(v) => decode_payouts(v.value()),
            None => Ok(vec![]),
        }
    }

    /// Height whose payouts mature into the note tree at `height`.
    pub fn matured_from(&self, height: u64) -> Option<u64> {
        let m = self.params.coinbase_maturity;
        (height > m).then(|| height - m)
    }

    fn matured_payouts(&self, height: u64) -> Result<Vec<Hash32>> {
        let Some(from) = self.matured_from(height) else {
            return Ok(vec![]);
        };
        Ok(consensus::payout_cms(from, &self.payouts_at(from)?)?)
    }

    /// Fully validates `block` on top of the current tip and applies it.
    fn connect(&self, block: &Block, now: u64) -> std::result::Result<(), ChainError> {
        let tip = self.tip();
        let hash = block.hash();
        let view = self.view()?;
        let history = self.branch_history(&tip.hash)?;
        let mmr_root = mmr::root(&view, tip.mmr_leaves());
        let matured = self.matured_payouts(block.header.height)?;
        let ctx = ParentContext {
            parent: &tip.header,
            history: &history,
            frontier: &tip.frontier,
            mmr_root,
            matured,
        };
        let fx = consensus::check_block(&self.params, &self.pow, &view, &ctx, block, now)?;
        let old_pool = view.pool();
        let old_accounts: Vec<(Hash32, Option<Account>)> = fx
            .reward
            .accounts
            .iter()
            .map(|(o, _)| (*o, view.account(o)))
            .collect();
        let leaf = MmrNode::leaf(&hash, block.header.difficulty);
        let mmr_nodes = mmr::append(
            &PendingMmr {
                view: &view,
                pending: vec![],
            },
            tip.mmr_leaves(),
            leaf,
        );
        drop(view);

        let height = block.header.height;
        let work = tip.work.saturating_add(block.header.difficulty);
        let mut undo = Writer::default();
        undo.bytes(&tip.frontier.to_bytes())
            .u32(fx.nullifiers.len() as u32);
        for n in &fx.nullifiers {
            undo.raw(n);
        }
        undo.u64(old_pool).u32(old_accounts.len() as u32);
        for (o, a) in &old_accounts {
            undo.raw(o);
            match a {
                Some(a) => undo.u8(1).raw(&a.to_bytes()),
                None => undo.u8(0),
            };
        }
        undo.u32(fx.shares.len() as u32);
        for s in &fx.shares {
            undo.raw(s);
        }
        let w = self.db.begin_write().map_err(anyhow::Error::from)?;
        (|| -> Result<()> {
            self.put_header(
                &w,
                &hash,
                &HeaderRecord {
                    header: block.header.clone(),
                    work,
                    status: STATUS_VALID,
                },
            )?;
            w.open_table(MAIN)?.insert(height, hash.as_slice())?;
            {
                let mut t = w.open_table(NULLIFIERS)?;
                for n in &fx.nullifiers {
                    t.insert(n.as_slice(), height)?;
                }
            }
            {
                let mut t = w.open_table(TREE)?;
                for (l, i, d) in &fx.new_nodes {
                    t.insert(tree_key(*l, *i), digest_to_bytes(d).as_slice())?;
                }
            }
            {
                let mut t = w.open_table(MMR)?;
                for (p, n) in &mmr_nodes {
                    t.insert(*p, n.to_bytes().as_slice())?;
                }
            }
            w.open_table(ROOTS)?
                .insert(block.header.note_root.as_slice(), height)?;
            {
                let mut t = w.open_table(TXINDEX)?;
                for tx in &block.txs {
                    t.insert(tx.txid().as_slice(), height)?;
                }
            }
            w.open_table(LEAFSTART)?.insert(height, tip.frontier.size)?;
            if height.is_multiple_of(FRONTIER_CHECKPOINT) {
                w.open_table(FRONTIERS)?
                    .insert(height, fx.frontier.to_bytes().as_slice())?;
            }
            {
                let mut t = w.open_table(SHARES)?;
                for s in &fx.shares {
                    t.insert(s.as_slice(), height)?;
                }
            }
            {
                let mut acc = w.open_table(ACCOUNTS)?;
                let mut due = w.open_table(DUE)?;
                for ((owner, new), (_, old)) in fx.reward.accounts.iter().zip(&old_accounts) {
                    if let Some(old) = old {
                        due.remove(due_key(old.since, owner).as_slice())?;
                    }
                    match new {
                        Some(a) => {
                            acc.insert(owner.as_slice(), a.to_bytes().as_slice())?;
                            due.insert(due_key(a.since, owner).as_slice(), 0)?;
                        }
                        None => {
                            acc.remove(owner.as_slice())?;
                        }
                    }
                }
            }
            if !fx.reward.payouts.is_empty() {
                w.open_table(PAYOUTS)?
                    .insert(height, encode_payouts(&fx.reward.payouts).as_slice())?;
            }
            w.open_table(UNDO)?
                .insert(hash.as_slice(), undo.0.as_slice())?;
            let mut meta = w.open_table(META)?;
            meta.insert("pool", fx.reward.pool.to_le_bytes().as_slice())?;
            meta.insert("tip", hash.as_slice())?;
            meta.insert("frontier", fx.frontier.to_bytes().as_slice())?;
            Ok(())
        })()?;
        w.commit().map_err(anyhow::Error::from)?;
        *self.tip.write() = Tip {
            header: block.header.clone(),
            hash,
            work,
            frontier: fx.frontier,
        };
        tracing::debug!(height, "connected block");
        Ok(())
    }

    /// Undoes the current tip block.
    fn disconnect_tip(&self) -> Result<Block> {
        let tip = self.tip();
        if tip.height() == 0 {
            bail!("cannot disconnect genesis");
        }
        let block = self.block(&tip.hash)?.context("tip block missing")?;
        let parent = self
            .header_record(&tip.header.prev)?
            .context("parent missing")?;
        let r = self.db.begin_read()?;
        let undo = r
            .open_table(UNDO)?
            .get(tip.hash.as_slice())?
            .context("undo record missing")?
            .value()
            .to_vec();
        drop(r);
        let mut rd = Reader::new(&undo);
        let frontier =
            Frontier::from_bytes(&rd.bytes(1 << 16)?).context("corrupt undo frontier")?;
        let n = rd.u32()? as usize;
        let mut nfs = Vec::with_capacity(n);
        for _ in 0..n {
            nfs.push(rd.array::<32>()?);
        }
        let old_pool = rd.u64()?;
        let n = rd.u32()? as usize;
        let mut old_accounts = Vec::with_capacity(n);
        for _ in 0..n {
            let owner: Hash32 = rd.array()?;
            let a = match rd.u8()? {
                1 => Some(Account::from_bytes(&rd.array::<16>()?).context("corrupt undo account")?),
                _ => None,
            };
            old_accounts.push((owner, a));
        }
        let n = rd.u32()? as usize;
        let mut shares = Vec::with_capacity(n);
        for _ in 0..n {
            shares.push(rd.array::<32>()?);
        }
        rd.finish()?;
        let height = tip.height();
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(NULLIFIERS)?;
            for nf in &nfs {
                t.remove(nf.as_slice())?;
            }
            let mut t = w.open_table(SHARES)?;
            for s in &shares {
                t.remove(s.as_slice())?;
            }
            {
                let mut acc = w.open_table(ACCOUNTS)?;
                let mut due = w.open_table(DUE)?;
                for (owner, old) in &old_accounts {
                    let current = acc
                        .get(owner.as_slice())?
                        .and_then(|v| Account::from_bytes(v.value()));
                    if let Some(c) = current {
                        due.remove(due_key(c.since, owner).as_slice())?;
                    }
                    match old {
                        Some(a) => {
                            acc.insert(owner.as_slice(), a.to_bytes().as_slice())?;
                            due.insert(due_key(a.since, owner).as_slice(), 0)?;
                        }
                        None => {
                            acc.remove(owner.as_slice())?;
                        }
                    }
                }
            }
            w.open_table(PAYOUTS)?.remove(height)?;
            let mut t = w.open_table(TREE)?;
            for (l, i) in tree::nodes_completed_by(frontier.size, tip.frontier.size) {
                t.remove(tree_key(l, i))?;
            }
            let mut roots = w.open_table(ROOTS)?;
            if roots
                .get(tip.header.note_root.as_slice())?
                .map(|v| v.value())
                == Some(height)
            {
                // Blocks that add no leaves keep their parent's root, which must stay a
                // valid anchor (at the parent's height) after this block is undone.
                if parent.header.note_root == tip.header.note_root {
                    roots.insert(tip.header.note_root.as_slice(), height - 1)?;
                } else {
                    roots.remove(tip.header.note_root.as_slice())?;
                }
            }
            {
                let mut t = w.open_table(TXINDEX)?;
                for tx in &block.txs {
                    t.remove(tx.txid().as_slice())?;
                }
            }
            w.open_table(MAIN)?.remove(height)?;
            w.open_table(LEAFSTART)?.remove(height)?;
            w.open_table(FRONTIERS)?.remove(height)?;
            w.open_table(UNDO)?.remove(tip.hash.as_slice())?;
            let mut meta = w.open_table(META)?;
            meta.insert("pool", old_pool.to_le_bytes().as_slice())?;
            meta.insert("tip", tip.header.prev.as_slice())?;
            meta.insert("frontier", frontier.to_bytes().as_slice())?;
        }
        w.commit()?;
        *self.tip.write() = Tip {
            header: parent.header,
            hash: tip.header.prev,
            work: parent.work,
            frontier,
        };
        Ok(block)
    }

    fn reorganize(&self, new_tip: Hash32, now: u64) -> std::result::Result<Outcome, ChainError> {
        // Branch from the new tip back to the main chain.
        let mut branch = Vec::new();
        let mut cur = new_tip;
        loop {
            if self.is_main(&cur)? {
                break;
            }
            let rec = self.header_record(&cur)?.context("missing branch header")?;
            if rec.status == STATUS_INVALID {
                return Err(ChainError::Invalid(CoreError::Invalid(
                    "branch contains an invalid block",
                )));
            }
            branch.push(cur);
            cur = rec.header.prev;
        }
        branch.reverse();
        // Every body must be at hand before the current chain is touched.
        for h in &branch {
            if !self.has_block(h)? {
                return Ok(Outcome::Orphan { missing: *h });
            }
        }
        let fork_height = self
            .header_record(&cur)?
            .context("fork point")?
            .header
            .height;
        let mut old = Vec::new();
        while self.tip().height() > fork_height {
            old.push(self.disconnect_tip()?);
        }
        for (i, h) in branch.iter().enumerate() {
            let result = match self.block(h)? {
                Some(block) => self.connect(&block, now),
                None => Err(ChainError::Storage(anyhow!("branch block missing"))),
            };
            if let Err(e) = result {
                tracing::warn!("reorg failed at branch block {}: {e}", i);
                if matches!(e, ChainError::Invalid(_)) {
                    for bad in &branch[i..] {
                        self.mark_invalid(bad)?;
                    }
                }
                while self.tip().height() > fork_height {
                    self.disconnect_tip()?;
                }
                for b in old.iter().rev() {
                    self.connect(b, now)
                        .map_err(|e| anyhow!("restoring previous chain failed: {e}"))?;
                }
                return Err(e);
            }
        }
        let reorg_depth = old.len() as u64;
        tracing::info!(depth = reorg_depth, "reorganized chain");
        let mut d = self.disconnected.lock();
        d.extend(old.into_iter().rev().flat_map(|b| b.txs));
        let excess = d.len().saturating_sub(MAX_DISCONNECTED_TXS);
        d.drain(..excess);
        drop(d);
        Ok(Outcome::Connected { reorg_depth })
    }

    /// Transactions of blocks dropped by reorganizations since the last call, oldest
    /// first; the node offers them to its mempool again.
    pub fn take_disconnected_txs(&self) -> Vec<krypton_core::Transaction> {
        std::mem::take(&mut *self.disconnected.lock())
    }

    /// Builds an unmined block on the current tip paying `miner`, crediting every
    /// includable share in `shares` and carrying `txs`.
    pub fn template(
        &self,
        miner: Hash32,
        shares: Vec<BlockHeader>,
        txs: Vec<krypton_core::Transaction>,
        timestamp: u64,
    ) -> Result<Block> {
        let tip = self.tip();
        let history = self.branch_history(&tip.hash)?;
        let view = self.view()?;
        let mmr_root = mmr::root(&view, tip.mmr_leaves());
        let matured = self.matured_payouts(tip.height() + 1)?;
        let ctx = ParentContext {
            parent: &tip.header,
            history: &history,
            frontier: &tip.frontier,
            mmr_root,
            matured,
        };
        Ok(consensus::build_template(
            &self.params,
            &view,
            &ctx,
            miner,
            shares,
            txs,
            timestamp,
        )?)
    }

    /// Checks a share for inclusion in the next block (context and proof of work).
    pub fn check_share(&self, s: &BlockHeader) -> std::result::Result<u128, CoreError> {
        let tip = self.tip();
        let history = self
            .branch_history(&tip.hash)
            .map_err(|_| CoreError::Invalid("storage"))?;
        let view = self.view().map_err(|_| CoreError::Invalid("storage"))?;
        let sc = consensus::ShareContext {
            height: tip.height() + 1,
            prev: tip.hash,
            difficulty: consensus::expected_difficulty(&self.params, &history),
            block_hash: None,
            history: &history,
        };
        let w = consensus::check_share_context(&view, &sc, s)?;
        consensus::check_share_pow(&self.pow, s)?;
        Ok(w)
    }

    /// Whether a share can still be credited on top of the current tip (no proof of work).
    pub fn share_includable(&self, s: &BlockHeader) -> bool {
        let tip = self.tip();
        let Ok(history) = self.branch_history(&tip.hash) else {
            return false;
        };
        let Ok(view) = self.view() else {
            return false;
        };
        let sc = consensus::ShareContext {
            height: tip.height() + 1,
            prev: tip.hash,
            difficulty: consensus::expected_difficulty(&self.params, &history),
            block_hash: None,
            history: &history,
        };
        consensus::check_share_context(&view, &sc, s).is_ok()
    }

    /// Unpaid reward account of a miner.
    pub fn account(&self, owner: &Hash32) -> Result<Option<Account>> {
        Ok(self.view()?.account(owner))
    }

    /// Reward pool balance (minted but not yet credited to shares).
    pub fn pool(&self) -> Result<u64> {
        Ok(self.view()?.pool())
    }

    /// Height at which a share was credited on the main chain.
    pub fn share_height(&self, hash: &Hash32) -> Result<Option<u64>> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(SHARES)?
            .get(hash.as_slice())?
            .map(|v| v.value()))
    }

    // ---- queries used by wallets, light clients and the explorer ----

    pub fn nullifier_height(&self, nf: &Hash32) -> Result<Option<u64>> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(NULLIFIERS)?
            .get(nf.as_slice())?
            .map(|v| v.value()))
    }

    /// Note tree size before block `height`.
    pub fn leaf_start(&self, height: u64) -> Result<Option<u64>> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(LEAFSTART)?.get(height)?.map(|v| v.value()))
    }

    /// Wallet view of a main-chain block, verifiable against its header.
    pub fn compact_block(&self, height: u64) -> Result<Option<krypton_light::CompactBlock>> {
        let Some(b) = self.block_at(height)? else {
            return Ok(None);
        };
        let leaf_start = self.leaf_start(height)?.unwrap_or(0);
        let matured_from = self.matured_from(height).unwrap_or(0);
        let matured = match self.matured_from(height) {
            Some(h) => self.payouts_at(h)?,
            None => vec![],
        };
        Ok(Some(krypton_light::CompactBlock {
            outputs: b
                .txs
                .iter()
                .flat_map(|t| t.outputs.iter().cloned())
                .collect(),
            nullifiers: b
                .txs
                .iter()
                .flat_map(|t| t.nullifiers.iter().map(|n| n.0))
                .collect(),
            payouts: self.payouts_at(height)?,
            header: b.header,
            leaf_start,
            matured,
            matured_from,
        }))
    }

    /// Frontier after block `height` (checkpoint heights and the tip only).
    pub fn frontier_at(&self, height: u64) -> Result<Option<Frontier>> {
        let tip = self.tip();
        if height == tip.height() {
            return Ok(Some(tip.frontier));
        }
        let r = self.db.begin_read()?;
        Ok(r.open_table(FRONTIERS)?
            .get(height)?
            .and_then(|v| Frontier::from_bytes(v.value())))
    }

    pub fn auth_path(&self, pos: u64) -> Result<(Vec<Digest>, Digest)> {
        let tip = self.tip();
        if pos >= tip.frontier.size {
            bail!("position not in tree");
        }
        let v = self.view()?;
        Ok((
            tree::auth_path(&v, tip.frontier.size, pos),
            tip.frontier.root(),
        ))
    }

    /// Leaves `[start, start+count)` of the note tree (subtree download for private witnesses).
    pub fn leaves(&self, start: u64, count: u64) -> Result<Vec<Hash32>> {
        let v = self.view()?;
        let size = self.tip().frontier.size;
        Ok((start..(start + count).min(size))
            .filter_map(|p| v.get(0, p).map(|d| digest_to_bytes(&d)))
            .collect())
    }

    /// Tree node at (level, index) for the current tree size.
    pub fn tree_node(&self, level: u8, index: u64) -> Result<Digest> {
        let v = self.view()?;
        Ok(tree::node_at(&v, self.tip().frontier.size, level, index))
    }

    /// MMR inclusion proof of block `leaf` in the MMR committed by the header at `at_height`.
    pub fn mmr_proof(&self, leaf: u64, at_height: u64) -> Result<Option<MmrProof>> {
        let v = self.view()?;
        Ok(mmr::prove(&v, at_height, leaf))
    }

    pub fn tx_height(&self, txid: &Hash32) -> Result<Option<u64>> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(TXINDEX)?
            .get(txid.as_slice())?
            .map(|v| v.value()))
    }

    pub fn put_address(&self, short_id: &Hash32, full: &[u8]) -> Result<()> {
        let w = self.db.begin_write()?;
        w.open_table(ADDRESSES)?.insert(short_id.as_slice(), full)?;
        w.commit()?;
        Ok(())
    }

    pub fn get_address(&self, short_id: &Hash32) -> Result<Option<Vec<u8>>> {
        let r = self.db.begin_read()?;
        Ok(r.open_table(ADDRESSES)?
            .get(short_id.as_slice())?
            .map(|v| v.value().to_vec()))
    }

    /// Validates a mempool transfer against the tip state.
    pub fn check_mempool_tx(
        &self,
        tx: &krypton_core::Transaction,
    ) -> std::result::Result<(), CoreError> {
        let st = consensus::check_tx_structure(tx)?;
        let view = self.view().map_err(|_| CoreError::Invalid("storage"))?;
        consensus::check_transfer_state(&view, tx, self.tip().height() + 1)?;
        consensus::verify_proof(tx, &st)
    }

    /// Cheap re-check of a pooled transfer after the tip changed (no proof verification).
    pub fn check_tx_state(
        &self,
        tx: &krypton_core::Transaction,
    ) -> std::result::Result<(), CoreError> {
        let view = self.view().map_err(|_| CoreError::Invalid("storage"))?;
        consensus::check_transfer_state(&view, tx, self.tip().height() + 1)
    }

    /// Coin supply issued through the tip.
    pub fn supply(&self) -> u64 {
        krypton_core::emission::issued_through(self.tip().height())
    }

    pub fn note_root_digest(&self) -> Digest {
        self.tip().frontier.root()
    }
}

#[cfg(test)]
mod tests;
