//! Consensus validation of headers, shares, transactions and blocks.

use crate::difficulty::next_difficulty;
use crate::emission::subsidy;
use crate::params::*;
use crate::reward::{self, Payout, RewardEffects, RewardView};
use crate::tree::Frontier;
use crate::types::*;
use crate::{CoreError, Hash32, Result};
use krypton_crypto::field::{Digest, digest_from_bytes, digest_to_bytes};
use krypton_pow::Verifier;
use krypton_zk::SpendPublic;
use rayon::prelude::*;

/// State a block is validated against (the main chain up to the block's parent).
pub trait StateView: RewardView {
    fn nullifier_spent(&self, nf: &Hash32) -> bool;
    /// Height of the main-chain block whose note root equals `root`.
    fn anchor_height(&self, root: &Hash32) -> Option<u64>;
    /// Hash of the main-chain block at `height` (up to the parent).
    fn main_hash(&self, height: u64) -> Option<Hash32>;
    /// Whether a share with this hash was already credited on the main chain.
    fn share_seen(&self, hash: &Hash32) -> bool;
}

/// Context of the parent block.
pub struct ParentContext<'a> {
    pub parent: &'a BlockHeader,
    /// (timestamp, difficulty) of recent blocks ending with the parent, oldest first.
    pub history: &'a [(u64, u128)],
    pub frontier: &'a Frontier,
    /// MMR root over all blocks up to and including the parent.
    pub mmr_root: Hash32,
    /// Commitments of the payouts made `maturity` blocks ago. Reward notes enter the
    /// note tree only once mature, so they cannot be spent earlier.
    pub matured: Vec<Hash32>,
}

/// Leaves appended to the note tree by a block, in order.
pub fn appended_leaves(matured: &[Hash32], b: &Block) -> Vec<Hash32> {
    let mut v = matured.to_vec();
    for t in &b.txs {
        v.extend(t.outputs.iter().map(|o| o.cm));
    }
    v
}

#[derive(Debug, Clone)]
pub struct BlockEffects {
    pub frontier: Frontier,
    pub new_nodes: Vec<(u8, u64, Digest)>,
    pub nullifiers: Vec<Hash32>,
    pub fees: u64,
    pub subsidy: u64,
    pub reward: RewardEffects,
    /// Hashes of the shares this block credits (its own header excluded).
    pub shares: Vec<Hash32>,
}

pub fn genesis_block(p: &ChainParams) -> Block {
    let header = BlockHeader {
        version: HEADER_VERSION,
        height: 0,
        prev: p.genesis_prev(),
        mmr_root: crate::mmr::root_from_peaks(&[], 0),
        tx_root: tx_root(&[]),
        share_root: reward::share_root(&[]),
        note_root: digest_to_bytes(&Frontier::new().root()),
        nullifier_digest: nullifier_digest(&[]),
        output_digest: output_digest(&[]),
        payout_digest: reward::payout_digest(&[]),
        miner: [0; 32],
        timestamp: p.genesis_time,
        difficulty: p.initial_difficulty,
        nonce: 0,
    };
    Block {
        header,
        shares: vec![],
        txs: vec![],
    }
}

pub fn median_time_past(history: &[(u64, u128)]) -> u64 {
    let n = history.len().min(MEDIAN_TIME_WINDOW);
    let mut ts: Vec<u64> = history[history.len() - n..].iter().map(|x| x.0).collect();
    ts.sort_unstable();
    ts.get(ts.len() / 2).copied().unwrap_or(0)
}

pub fn expected_difficulty(p: &ChainParams, history: &[(u64, u128)]) -> u128 {
    next_difficulty(history, p.initial_difficulty)
}

fn canonical_owner(o: &Hash32) -> bool {
    digest_from_bytes(o).is_ok()
}

/// Contextual header checks including proof of work.
pub fn check_header(
    p: &ChainParams,
    pow: &Verifier,
    ctx: &ParentContext,
    h: &BlockHeader,
    now: u64,
) -> Result<()> {
    let bad = |m| Err(CoreError::Invalid(m));
    if h.version != HEADER_VERSION {
        return bad("unsupported header version");
    }
    if h.height != ctx.parent.height + 1 || h.prev != ctx.parent.hash() {
        return bad("header does not extend its parent");
    }
    if h.timestamp <= median_time_past(ctx.history) {
        return bad("timestamp not after median time past");
    }
    if h.timestamp > now + MAX_FUTURE_DRIFT {
        return Err(CoreError::Future);
    }
    if h.difficulty != expected_difficulty(p, ctx.history) {
        return bad("wrong difficulty");
    }
    if h.mmr_root != ctx.mmr_root {
        return bad("wrong MMR root");
    }
    if !canonical_owner(&h.miner) {
        return bad("non-canonical miner owner");
    }
    if !pow.verify(h.height, &h.pow_hash(), h.nonce, h.difficulty) {
        return bad("insufficient proof of work");
    }
    Ok(())
}

/// Where a share would be credited: a block at `height` on top of `prev`, whose
/// difficulty is `difficulty`; `history` ends with the block at `height - 1`.
pub struct ShareContext<'a> {
    pub height: u64,
    pub prev: Hash32,
    pub difficulty: u128,
    pub block_hash: Option<Hash32>,
    pub history: &'a [(u64, u128)],
}

/// Checks that `s` may be credited in the block described by `sc` (everything but
/// proof of work). Returns the share's weight.
pub fn check_share_context(
    view: &impl StateView,
    sc: &ShareContext,
    s: &BlockHeader,
) -> Result<u128> {
    let bad = |m| Err(CoreError::Invalid(m));
    if s.version != HEADER_VERSION {
        return bad("unsupported share version");
    }
    if s.height == 0 || s.height > sc.height || s.height + SHARE_WINDOW <= sc.height {
        return bad("share outside the inclusion window");
    }
    if !canonical_owner(&s.miner) {
        return bad("non-canonical share owner");
    }
    let hash = s.hash();
    if Some(hash) == sc.block_hash || view.share_seen(&hash) {
        return bad("share already credited");
    }
    if s.height == sc.height {
        if s.prev != sc.prev || s.difficulty != sc.difficulty {
            return bad("share does not build on the parent");
        }
    } else {
        if view.main_hash(s.height - 1) != Some(s.prev) {
            return bad("share does not build on the main chain");
        }
        if view.main_hash(s.height) == Some(hash) {
            return bad("main-chain blocks are not shares");
        }
        let back = (sc.height - 1 - s.height) as usize;
        let Some(&(_, d)) = sc
            .history
            .len()
            .checked_sub(back + 1)
            .and_then(|i| sc.history.get(i))
        else {
            return bad("share history unavailable");
        };
        if s.difficulty != d {
            return bad("wrong share difficulty");
        }
    }
    Ok(s.difficulty)
}

pub fn check_share_pow(pow: &Verifier, s: &BlockHeader) -> Result<()> {
    if pow.verify(
        s.height,
        &s.pow_hash(),
        s.nonce,
        reward::share_difficulty(s.difficulty),
    ) {
        Ok(())
    } else {
        Err(CoreError::Invalid("share has insufficient proof of work"))
    }
}

/// Structure checks that need no chain state. Returns the proof statement.
pub fn check_tx_structure(tx: &Transaction) -> Result<SpendPublic> {
    let bad = |m| Err(CoreError::Invalid(m));
    match (tx.version, &tx.lock) {
        (TX_VERSION, None) => {}
        (TX_VERSION_LOCK, Some(l)) => {
            l.check([&tx.outputs[0].cm, &tx.outputs[1].cm], tx.expiry_height)?
        }
        _ => return bad("unsupported transaction version"),
    }
    if tx
        .outputs
        .iter()
        .any(|o| o.ciphertext.len() != NOTE_CIPHERTEXT_LEN)
    {
        return bad("wrong note ciphertext length");
    }
    if tx.nullifiers[0] == tx.nullifiers[1] {
        return bad("duplicate nullifier in transaction");
    }
    if tx.outputs[0].cm == tx.outputs[1].cm {
        return bad("duplicate output commitment");
    }
    if tx.size() > MAX_TX_BYTES {
        return bad("transaction too large");
    }
    if tx.kind != TxKind::Transfer || tx.mint != 0 {
        return bad("transactions may not mint");
    }
    if tx.fee > krypton_zk::layout::MAX_VALUE {
        return bad("amount out of range");
    }
    tx.statement()
}

pub fn verify_proof(tx: &Transaction, st: &SpendPublic) -> Result<()> {
    krypton_zk::verify(st, &tx.proof).map_err(|_| CoreError::Invalid("invalid spend proof"))
}

/// State checks for a transfer to be included at `height`.
pub fn check_transfer_state(
    p: &ChainParams,
    view: &impl StateView,
    tx: &Transaction,
    height: u64,
) -> Result<()> {
    if tx.expiry_height != 0 && tx.expiry_height < height {
        return Err(CoreError::Invalid("transaction expired"));
    }
    if let Some(l) = &tx.lock {
        l.check_height(height, p.lock_height)?;
    }
    match view.anchor_height(&tx.anchor) {
        Some(h) if h + ANCHOR_WINDOW >= height => {}
        _ => return Err(CoreError::Invalid("unknown or expired anchor")),
    }
    if tx.nullifiers.iter().any(|n| view.nullifier_spent(&n.0)) {
        return Err(CoreError::DoubleSpend);
    }
    Ok(())
}

/// Rewards of a block: its own header is a share of its miner.
fn block_rewards(
    p: &ChainParams,
    view: &impl StateView,
    h: &BlockHeader,
    weights: &[(Hash32, u128)],
    fees: u64,
) -> Result<RewardEffects> {
    let mut all = Vec::with_capacity(weights.len() + 1);
    all.push((h.miner, h.difficulty));
    all.extend_from_slice(weights);
    let inflow = subsidy(h.height)
        .checked_add(fees)
        .ok_or(CoreError::Invalid("reward overflow"))?;
    reward::apply(
        view,
        h.height,
        h.difficulty,
        inflow,
        &all,
        p.payout_interval,
    )
}

/// Checks that the body is exactly the one the header commits to (no chain state).
/// A mismatch says nothing about the header itself: its real body may be valid, so a
/// node must reject such a block without remembering the header as invalid.
pub fn check_body_commitments(b: &Block) -> Result<()> {
    let h = &b.header;
    if b.shares.len() > MAX_SHARES_PER_BLOCK {
        return Err(CoreError::Invalid("too many shares"));
    }
    if h.tx_root != tx_root(&b.txs)
        || h.nullifier_digest != nullifier_digest(&b.txs)
        || h.output_digest != output_digest(&b.txs)
        || h.share_root != reward::share_root(&b.shares)
    {
        return Err(CoreError::Invalid("block commitments do not match body"));
    }
    Ok(())
}

/// Full validation of a block on top of `ctx`. Returns the state changes to apply.
pub fn check_block(
    p: &ChainParams,
    pow: &Verifier,
    view: &impl StateView,
    ctx: &ParentContext,
    b: &Block,
    now: u64,
) -> Result<BlockEffects> {
    check_header(p, pow, ctx, &b.header, now)?;
    let bad = |m| Err(CoreError::Invalid(m));
    let h = &b.header;
    if b.to_bytes().len() > MAX_BLOCK_BYTES {
        return bad("block too large");
    }
    check_body_commitments(b)?;
    let share_hashes: Vec<Hash32> = b.shares.iter().map(|s| s.hash()).collect();
    if share_hashes.windows(2).any(|w| w[0] >= w[1]) {
        return bad("shares must be strictly ordered by hash");
    }
    let sc = ShareContext {
        height: h.height,
        prev: h.prev,
        difficulty: h.difficulty,
        block_hash: Some(h.hash()),
        history: ctx.history,
    };
    let weights: Vec<(Hash32, u128)> = b
        .shares
        .iter()
        .map(|s| check_share_context(view, &sc, s).map(|w| (s.miner, w)))
        .collect::<Result<_>>()?;
    let statements: Vec<SpendPublic> = b
        .txs
        .iter()
        .map(check_tx_structure)
        .collect::<Result<_>>()?;

    let mut seen = std::collections::HashSet::new();
    let mut fees: u64 = 0;
    for t in &b.txs {
        check_transfer_state(p, view, t, h.height)?;
        fees = fees
            .checked_add(t.fee)
            .ok_or(CoreError::Invalid("fee overflow"))?;
        for n in &t.nullifiers {
            if !seen.insert(n.0) {
                return Err(CoreError::DoubleSpend);
            }
        }
    }
    if crate::emission::issued_through(h.height) > crate::emission::MAX_SUPPLY {
        return bad("supply cap exceeded");
    }
    let reward = block_rewards(p, view, h, &weights, fees)?;
    if h.payout_digest != reward::payout_digest(&reward.payouts) {
        return bad("wrong payouts");
    }
    b.shares
        .par_iter()
        .try_for_each(|s| check_share_pow(pow, s))?;
    b.txs
        .par_iter()
        .zip(statements.par_iter())
        .try_for_each(|(t, st)| verify_proof(t, st))?;

    let mut frontier = ctx.frontier.clone();
    let mut new_nodes = Vec::new();
    for cm in appended_leaves(&ctx.matured, b) {
        let leaf =
            digest_from_bytes(&cm).map_err(|_| CoreError::Invalid("non-canonical commitment"))?;
        new_nodes.extend(frontier.append(leaf));
    }
    if digest_to_bytes(&frontier.root()) != h.note_root {
        return bad("wrong note root");
    }
    Ok(BlockEffects {
        frontier,
        new_nodes,
        nullifiers: seen.into_iter().collect(),
        fees,
        subsidy: subsidy(h.height),
        reward,
        shares: share_hashes,
    })
}

/// Commitments of the payouts a block made, in tree order.
pub fn payout_cms(height: u64, payouts: &[Payout]) -> Result<Vec<Hash32>> {
    payouts
        .iter()
        .enumerate()
        .map(|(i, p)| reward::payout_cm(height, i as u32, p))
        .collect()
}

/// Builds an unmined block paying `miner` on top of `ctx`. Shares that are not
/// includable are dropped; the rest are deduplicated and ordered by hash.
pub fn build_template(
    p: &ChainParams,
    view: &impl StateView,
    ctx: &ParentContext,
    miner: Hash32,
    mut shares: Vec<BlockHeader>,
    txs: Vec<Transaction>,
    timestamp: u64,
) -> Result<Block> {
    if !canonical_owner(&miner) {
        return Err(CoreError::Invalid("non-canonical miner owner"));
    }
    let height = ctx.parent.height + 1;
    let difficulty = expected_difficulty(p, ctx.history);
    let sc = ShareContext {
        height,
        prev: ctx.parent.hash(),
        difficulty,
        block_hash: None,
        history: ctx.history,
    };
    // With more pending shares than fit, the oldest go first so none expires uncredited;
    // the body itself is then ordered by hash, as consensus requires.
    shares.sort_by_key(|s| (s.height, s.hash()));
    shares.dedup_by_key(|s| s.hash());
    let mut picked = Vec::new();
    for s in shares {
        if picked.len() >= MAX_SHARES_PER_BLOCK {
            break;
        }
        if let Ok(w) = check_share_context(view, &sc, &s) {
            picked.push((s, w));
        }
    }
    picked.sort_by_key(|(s, _)| s.hash());
    let weights: Vec<_> = picked.iter().map(|(s, w)| (s.miner, *w)).collect();
    let shares: Vec<BlockHeader> = picked.into_iter().map(|(s, _)| s).collect();
    let fees: u64 = txs.iter().map(|t| t.fee).sum();
    let mut header = BlockHeader {
        version: HEADER_VERSION,
        height,
        prev: ctx.parent.hash(),
        mmr_root: ctx.mmr_root,
        tx_root: tx_root(&txs),
        share_root: reward::share_root(&shares),
        note_root: [0; 32],
        nullifier_digest: nullifier_digest(&txs),
        output_digest: output_digest(&txs),
        payout_digest: [0; 32],
        miner,
        timestamp: timestamp.max(median_time_past(ctx.history) + 1),
        difficulty,
        nonce: 0,
    };
    let reward = block_rewards(p, view, &header, &weights, fees)?;
    header.payout_digest = reward::payout_digest(&reward.payouts);
    let block = Block {
        header,
        shares,
        txs,
    };
    let mut frontier = ctx.frontier.clone();
    for cm in appended_leaves(&ctx.matured, &block) {
        frontier.append(
            digest_from_bytes(&cm).map_err(|_| CoreError::Invalid("non-canonical commitment"))?,
        );
    }
    let mut block = block;
    block.header.note_root = digest_to_bytes(&frontier.root());
    Ok(block)
}
