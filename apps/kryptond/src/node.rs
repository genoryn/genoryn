//! Node state and peer protocol logic (sync, relay).

use crate::mempool::Mempool;
use anyhow::Result;
use krypton_core::params::{MAX_SHARES_PER_BLOCK, SHARE_WINDOW};
use krypton_core::types::{Block, BlockHeader, Transaction};
use krypton_core::{CoreError, Hash32};
use krypton_p2p::{Event, Hello, Message, Net, PeerId};
use krypton_store::{Chain, ChainError, Outcome};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

pub const USER_AGENT: &str = concat!("kryptond/", env!("CARGO_PKG_VERSION"));

pub struct Job {
    pub block: Block,
    pub created: Instant,
}

/// Shares waiting to be credited by a block.
#[derive(Default)]
pub struct SharePool {
    pub shares: HashMap<Hash32, (BlockHeader, Instant)>,
    pub revision: u64,
}

const MAX_POOL_SHARES: usize = 4096;

/// Per-owner share counts over the recent blocks, computed once per tip:
/// (tip hash, counts, blocks covered).
pub type RecentShares = (Hash32, HashMap<Hash32, u64>, u64);

impl SharePool {
    /// Oldest first (closest to leaving the inclusion window), at most one block's worth.
    pub fn for_template(&self) -> Vec<BlockHeader> {
        let mut v: Vec<&BlockHeader> = self.shares.values().map(|(h, _)| h).collect();
        v.sort_by_key(|h| (h.height, h.hash()));
        v.into_iter().take(MAX_SHARES_PER_BLOCK).cloned().collect()
    }
    pub fn recent(&self, n: usize) -> Vec<BlockHeader> {
        let mut v: Vec<&(BlockHeader, Instant)> = self.shares.values().collect();
        v.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        v.into_iter().take(n).map(|(h, _)| h.clone()).collect()
    }
}

pub enum ShareOutcome {
    Accepted,
    Known,
}

/// Why a transaction was not added to the pool.
#[derive(Debug)]
pub enum TxReject {
    /// Not acceptable now: fee policy, conflicts, a spent nullifier, a stale anchor.
    Rejected(String),
    /// Can never be valid (malformed, or the proof fails).
    Invalid(String),
}

impl std::fmt::Display for TxReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxReject::Rejected(m) | TxReject::Invalid(m) => f.write_str(m),
        }
    }
}

#[derive(Debug)]
pub enum ShareError {
    /// Not creditable on top of the current tip (stale, duplicate, wrong context).
    Stale(String),
    /// Fails the share target: the sender did no work.
    BadWork,
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareError::Stale(m) => write!(f, "share not creditable: {m}"),
            ShareError::BadWork => write!(f, "share does not meet the share target"),
        }
    }
}

pub struct Node {
    pub chain: Arc<Chain>,
    pub net: Net,
    pub mempool: Mutex<Mempool>,
    pub shares: Mutex<SharePool>,
    pub hashrate_cache: Mutex<Option<(Hash32, f64)>>,
    pub stats: Arc<Mutex<crate::stats::Stats>>,
    /// Latest `Stats::summary`, readable without waiting for a refresh.
    pub stats_summary: Arc<Mutex<serde_json::Value>>,
    pub recent_shares: Mutex<Option<RecentShares>>,
    pub events: broadcast::Sender<String>,
    pub jobs: Mutex<HashMap<u64, Job>>,
    pub templates: Mutex<HashMap<String, (Hash32, u64, Instant, u64)>>,
    pub template_budget: Mutex<Vec<Instant>>,
    /// Recent address-directory writes (bounds disk use from the public API).
    pub address_budget: Mutex<Vec<Instant>>,
    pub nonce: u64,
    pub started: Instant,
    peer_heights: Mutex<HashMap<PeerId, u64>>,
    in_flight: Mutex<HashSet<Hash32>>,
    /// Our own transactions in the stem phase: (txid, first seen).
    stem: Mutex<Vec<(Hash32, Instant)>>,
    pub listen_port: u16,
}

impl Node {
    pub fn new(chain: Arc<Chain>, net: Net, listen_port: u16) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            chain,
            net,
            mempool: Mutex::new(Mempool::default()),
            shares: Mutex::new(SharePool::default()),
            hashrate_cache: Mutex::new(None),
            stats: Arc::new(Mutex::new(crate::stats::Stats::default())),
            stats_summary: Arc::new(Mutex::new(serde_json::Value::Null)),
            recent_shares: Mutex::new(None),
            events,
            jobs: Mutex::new(HashMap::new()),
            templates: Mutex::new(HashMap::new()),
            template_budget: Mutex::new(Vec::new()),
            address_budget: Mutex::new(Vec::new()),
            nonce: rand::random(),
            started: Instant::now(),
            peer_heights: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashSet::new()),
            stem: Mutex::new(Vec::new()),
            listen_port,
        })
    }

    fn hello(&self) -> Hello {
        let tip = self.chain.tip();
        Hello {
            version: 1,
            chain_id: self.chain.params.chain_id.into(),
            genesis: self.chain.genesis.hash(),
            height: tip.height(),
            best: tip.hash,
            user_agent: USER_AGENT.into(),
            listen_port: self.listen_port,
            nonce: self.nonce,
        }
    }

    pub fn best_peer_height(&self) -> u64 {
        self.peer_heights
            .lock()
            .values()
            .copied()
            .max()
            .unwrap_or(0)
    }

    fn request_hashes(&self, peer: PeerId) {
        if let Ok(locator) = self.chain.locator() {
            self.net
                .send(peer, &Message::GetHashes { locator, max: 500 });
        }
    }

    /// Extends the statistics cache off the async runtime.
    pub fn refresh_stats(&self) {
        let (chain, stats, summary) = (
            self.chain.clone(),
            self.stats.clone(),
            self.stats_summary.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let mut s = stats.lock();
            s.refresh(&chain);
            *summary.lock() = s.summary();
        });
    }

    pub async fn run(self: Arc<Self>, mut rx: mpsc::Receiver<Event>) {
        let me = self.clone();
        tokio::spawn(async move { me.housekeeping().await });
        self.refresh_stats();
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::Connected(p) => self.net.send(p, &Message::Hello(self.hello())),
                Event::Disconnected(p) => {
                    self.peer_heights.lock().remove(&p);
                }
                Event::Message(p, m) => {
                    if let Err(e) = self.handle(p, m).await {
                        tracing::debug!("peer {p}: {e}");
                    }
                }
            }
        }
    }

    async fn housekeeping(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let ours = self.chain.tip().height();
            let behind: Vec<PeerId> = self
                .peer_heights
                .lock()
                .iter()
                .filter(|(_, h)| **h > ours)
                .map(|(p, _)| *p)
                .collect();
            if let Some(p) = behind.first() {
                self.in_flight.lock().clear();
                self.request_hashes(*p);
            }
            // Dandelion-style fluff: after the stem embargo, announce our own txs to everyone.
            let due: Vec<Hash32> = {
                let mut s = self.stem.lock();
                let due = s
                    .iter()
                    .filter(|(_, t)| t.elapsed() > Duration::from_secs(20))
                    .map(|x| x.0)
                    .collect();
                s.retain(|(_, t)| t.elapsed() <= Duration::from_secs(20));
                due
            };
            let due: Vec<Hash32> = due
                .into_iter()
                .filter(|id| self.mempool.lock().contains(id))
                .collect();
            if !due.is_empty() {
                self.net.broadcast(
                    &Message::Inv {
                        blocks: vec![],
                        txs: due,
                    },
                    None,
                );
            }
            self.jobs
                .lock()
                .retain(|_, j| j.created.elapsed() < Duration::from_secs(600));
            if self.net.ready_peers().len() < 3 {
                self.net.broadcast(&Message::GetAddr, None);
            }
        }
    }

    async fn handle(self: &Arc<Self>, p: PeerId, m: Message) -> Result<()> {
        match m {
            Message::Hello(h) => {
                if h.nonce == self.nonce {
                    self.net.disconnect(p);
                    return Ok(());
                }
                if h.genesis != self.chain.genesis.hash()
                    || h.chain_id != self.chain.params.chain_id
                {
                    self.net.disconnect(p);
                    return Ok(());
                }
                self.peer_heights.lock().insert(p, h.height);
                self.net.set_hello(p, h.clone());
                if h.height > self.chain.tip().height() {
                    self.request_hashes(p);
                }
                self.net.send(p, &Message::GetAddr);
                let pending: Vec<Vec<u8>> = self
                    .shares
                    .lock()
                    .recent(krypton_p2p::message::MAX_SHARES_PER_MESSAGE)
                    .iter()
                    .map(|s| s.to_bytes())
                    .collect();
                if !pending.is_empty() {
                    self.net.send(p, &Message::Shares(pending));
                }
            }
            Message::Shares(v) => {
                let total = v.len();
                let mut stale = 0usize;
                for raw in v {
                    let Ok(s) = BlockHeader::from_bytes(&raw) else {
                        self.net.ban(p);
                        return Ok(());
                    };
                    match self.accept_share(s, Some(p)).await {
                        Ok(_) => {}
                        Err(ShareError::BadWork) => {
                            tracing::warn!("peer {p} sent a share without work");
                            self.net.ban(p);
                            return Ok(());
                        }
                        Err(ShareError::Stale(e)) => {
                            stale += 1;
                            tracing::debug!("share from {p}: {e}");
                        }
                    }
                }
                // Honest peers send recent shares; a flood of uncreditable ones is abuse.
                if total >= 64 && stale * 2 > total {
                    tracing::warn!("peer {p} sent {stale}/{total} uncreditable shares");
                    self.net.disconnect(p);
                }
            }
            Message::Ping(n) => self.net.send(p, &Message::Pong(n)),
            Message::Pong(_) => {}
            Message::GetHashes { locator, max } => {
                let v = self
                    .chain
                    .hashes_after(&locator, (max as usize).min(2000))?;
                self.net.send(p, &Message::Hashes(v));
            }
            Message::Hashes(v) => {
                let mut want = Vec::new();
                for h in v {
                    if !self.chain.has_block(&h)? && self.in_flight.lock().insert(h) {
                        want.push(h);
                    }
                }
                for c in want.chunks(32) {
                    self.net.send(p, &Message::GetBlocks(c.to_vec()));
                }
            }
            Message::GetBlocks(v) => {
                // Requests beyond what the peer is reading are dropped; it asks again.
                for h in v.iter().take(64) {
                    if self.net.queued_bytes(p) > krypton_p2p::SOFT_QUEUED_BYTES {
                        break;
                    }
                    if let Some(b) = self.chain.block(h)? {
                        self.net.send(p, &Message::Block(b.to_bytes()));
                    }
                }
            }
            Message::Block(raw) => {
                let block = match Block::from_bytes(&raw) {
                    Ok(b) => b,
                    Err(_) => {
                        self.net.ban(p);
                        return Ok(());
                    }
                };
                let hash = block.hash();
                self.in_flight.lock().remove(&hash);
                match self.submit_block(block.clone()).await {
                    Ok(Outcome::Connected { .. }) => {
                        let h = block.header.height;
                        let mut ph = self.peer_heights.lock();
                        let e = ph.entry(p).or_insert(0);
                        *e = (*e).max(h);
                        drop(ph);
                        if self.in_flight.lock().is_empty()
                            && self.best_peer_height() > self.chain.tip().height()
                        {
                            self.request_hashes(p);
                        }
                    }
                    Ok(Outcome::Orphan { .. }) => self.request_hashes(p),
                    Ok(_) => {}
                    Err(ChainError::Invalid(CoreError::Future)) => {}
                    Err(ChainError::Invalid(e)) => {
                        tracing::warn!("invalid block from peer {p}: {e}");
                        self.net.ban(p);
                    }
                    Err(e) => tracing::error!("storage error: {e}"),
                }
            }
            Message::Inv { blocks, txs } => {
                let mut want_b = Vec::new();
                for h in blocks.into_iter().take(500) {
                    if !self.chain.has_block(&h)? && self.in_flight.lock().insert(h) {
                        want_b.push(h);
                    }
                }
                if !want_b.is_empty() {
                    self.net.send(p, &Message::GetBlocks(want_b));
                }
                let want_t: Vec<Hash32> = {
                    let mp = self.mempool.lock();
                    txs.into_iter()
                        .take(500)
                        .filter(|t| !mp.contains(t))
                        .collect()
                };
                let want_t: Vec<Hash32> = want_t
                    .into_iter()
                    .filter(|t| !self.chain.tx_height(t).ok().flatten().is_some())
                    .collect();
                if !want_t.is_empty() {
                    self.net.send(p, &Message::GetTxs(want_t));
                }
            }
            Message::GetTxs(v) => {
                let mp = self.mempool.lock();
                for id in v.iter().take(500) {
                    if self.net.queued_bytes(p) > krypton_p2p::SOFT_QUEUED_BYTES {
                        break;
                    }
                    if let Some(e) = mp.txs.get(id) {
                        self.net.send(p, &Message::Tx(e.tx.to_bytes()));
                    }
                }
            }
            Message::Tx(raw) => {
                let Ok(tx) = Transaction::from_bytes(&raw) else {
                    self.net.ban(p);
                    return Ok(());
                };
                match self.accept_tx(tx).await {
                    Ok(id) => self.net.broadcast(
                        &Message::Inv {
                            blocks: vec![],
                            txs: vec![id],
                        },
                        Some(p),
                    ),
                    // Honest nodes only relay transactions whose proofs they verified.
                    Err(TxReject::Invalid(e)) => {
                        tracing::warn!("peer {p} sent an invalid transaction: {e}");
                        self.net.ban(p);
                    }
                    Err(e) => tracing::debug!("rejected tx from {p}: {e}"),
                }
            }
            Message::GetAddr => self.net.send(p, &Message::Addr(self.net.addrs())),
            Message::Addr(v) => self.net.add_addrs(v),
        }
        Ok(())
    }

    /// Validates and stores a block (from a peer or a miner); relays it on success.
    pub async fn submit_block(
        self: &Arc<Self>,
        block: Block,
    ) -> std::result::Result<Outcome, ChainError> {
        let chain = self.chain.clone();
        let b = block.clone();
        let out = tokio::task::spawn_blocking(move || chain.process_block(&b, krypton_core::now()))
            .await
            .map_err(|e| ChainError::Storage(anyhow::anyhow!("{e}")))??;
        if let Outcome::Connected { reorg_depth } = out {
            self.refresh_stats();
            let tip = self.chain.tip();
            {
                let mut mp = self.mempool.lock();
                for t in &block.txs {
                    mp.remove(&t.txid());
                }
                let chain = self.chain.clone();
                mp.retain_valid(|t| chain.check_tx_state(t).is_ok());
                // Transfers from blocks a reorganization dropped were valid (proof
                // included); those still valid on the new chain wait for the next block.
                for t in chain.take_disconnected_txs() {
                    let id = t.txid();
                    if !mp.contains(&id)
                        && chain.tx_height(&id).ok().flatten().is_none()
                        && chain.check_tx_state(&t).is_ok()
                        && mp.precheck(&t).is_ok()
                    {
                        mp.insert(t);
                    }
                }
            }
            {
                let chain = self.chain.clone();
                let next = tip.height() + 1;
                let mut sp = self.shares.lock();
                sp.shares.retain(|h, (s, _)| {
                    s.height + SHARE_WINDOW > next
                        && !chain.share_height(h).ok().flatten().is_some()
                });
                sp.revision += 1;
            }
            self.net.broadcast(
                &Message::Inv {
                    blocks: vec![tip.hash],
                    txs: vec![],
                },
                None,
            );
            let ev = serde_json::json!({
                "type": "block",
                "height": tip.height(),
                "hash": hex::encode(tip.hash),
                "txs": block.txs.len(),
                "reorg_depth": reorg_depth,
                "timestamp": tip.header.timestamp,
            });
            let _ = self.events.send(ev.to_string());
            if tip.height().is_multiple_of(100) || reorg_depth > 0 {
                tracing::info!(height = tip.height(), reorg_depth, "new tip");
            } else {
                tracing::debug!(height = tip.height(), "new tip");
            }
        }
        Ok(out)
    }

    /// Validates a share against the tip, pools it and relays it to peers.
    pub async fn accept_share(
        self: &Arc<Self>,
        s: BlockHeader,
        from: Option<PeerId>,
    ) -> std::result::Result<ShareOutcome, ShareError> {
        let hash = s.hash();
        if self.shares.lock().shares.contains_key(&hash) {
            return Ok(ShareOutcome::Known);
        }
        // Cheap window check before any storage access.
        let next = self.chain.tip().height() + 1;
        if s.height == 0 || s.height > next || s.height + SHARE_WINDOW <= next {
            return Err(ShareError::Stale("outside the inclusion window".into()));
        }
        let chain = self.chain.clone();
        let s2 = s.clone();
        tokio::task::spawn_blocking(move || {
            if !chain.share_includable(&s2) {
                return Err(ShareError::Stale(
                    "outside the window, already credited or not on the main chain".into(),
                ));
            }
            krypton_core::consensus::check_share_pow(&chain.pow, &s2)
                .map_err(|_| ShareError::BadWork)
        })
        .await
        .map_err(|e| ShareError::Stale(e.to_string()))??;
        {
            let mut sp = self.shares.lock();
            if sp.shares.len() >= MAX_POOL_SHARES
                && let Some(oldest) = sp
                    .shares
                    .iter()
                    .min_by_key(|(_, (h, t))| (h.height, *t))
                    .map(|(k, _)| *k)
            {
                sp.shares.remove(&oldest);
            }
            if sp
                .shares
                .insert(hash, (s.clone(), Instant::now()))
                .is_some()
            {
                return Ok(ShareOutcome::Known);
            }
            sp.revision += 1;
        }
        self.net
            .broadcast(&Message::Shares(vec![s.to_bytes()]), from);
        let _ = self.events.send(
            serde_json::json!({
                "type": "share",
                "hash": hex::encode(hash),
                "height": s.height,
                "miner": hex::encode(s.miner),
            })
            .to_string(),
        );
        Ok(ShareOutcome::Accepted)
    }

    /// Validates a transaction for the pool: cheap checks first, the proof last and off
    /// the async runtime.
    pub async fn accept_tx(
        self: &Arc<Self>,
        tx: Transaction,
    ) -> std::result::Result<Hash32, TxReject> {
        let id = tx.txid();
        if self.mempool.lock().contains(&id) {
            return Err(TxReject::Rejected("already in pool".into()));
        }
        let st = krypton_core::consensus::check_tx_structure(&tx)
            .map_err(|e| TxReject::Invalid(e.to_string()))?;
        self.mempool
            .lock()
            .precheck(&tx)
            .map_err(TxReject::Rejected)?;
        let chain = self.chain.clone();
        let t2 = tx.clone();
        tokio::task::spawn_blocking(move || {
            chain
                .check_tx_state(&t2)
                .map_err(|e| TxReject::Rejected(e.to_string()))?;
            krypton_core::consensus::verify_proof(&t2, &st)
                .map_err(|e| TxReject::Invalid(e.to_string()))
        })
        .await
        .map_err(|e| TxReject::Rejected(e.to_string()))??;
        let mut mp = self.mempool.lock();
        mp.precheck(&tx).map_err(TxReject::Rejected)?;
        mp.insert(tx);
        drop(mp);
        let _ = self
            .events
            .send(serde_json::json!({"type": "tx", "txid": hex::encode(id)}).to_string());
        Ok(id)
    }

    /// Our own transaction: stem to one random peer, fluff to all after the embargo.
    pub fn relay_local(&self, id: Hash32) {
        use rand::seq::SliceRandom;
        let peers = self.net.ready_peers();
        if let Some(p) = peers.choose(&mut rand::thread_rng()) {
            self.net.send(
                *p,
                &Message::Inv {
                    blocks: vec![],
                    txs: vec![id],
                },
            );
            self.stem.lock().push((id, Instant::now()));
        }
    }
}
