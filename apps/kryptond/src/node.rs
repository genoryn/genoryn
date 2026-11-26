//! Node state and peer protocol logic (sync, relay).

use crate::mempool::Mempool;
use anyhow::Result;
use krypton_core::params::{MAX_SHARES_PER_BLOCK, SHARE_WINDOW};
use krypton_core::types::{Block, BlockHeader, Transaction};
use krypton_core::{CoreError, Hash32};
use krypton_p2p::{Event, Hello, Message, Net, PeerId};
use krypton_store::{Chain, ChainError, Outcome};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

pub const USER_AGENT: &str = concat!("kryptond/", env!("CARGO_PKG_VERSION"));

pub struct Job {
    pub block: Block,
    pub created: Instant,
    /// Encoded size, for the jobs memory budget.
    pub bytes: usize,
}

/// Shares waiting to be credited by a block.
#[derive(Default)]
pub struct SharePool {
    pub shares: HashMap<Hash32, (BlockHeader, Instant)>,
    pub revision: u64,
}

const MAX_POOL_SHARES: usize = 4096;
const IN_FLIGHT_TTL: Duration = Duration::from_secs(30);
const MAX_IN_FLIGHT: usize = 4096;
const MAX_IN_FLIGHT_PER_PEER: usize = 1024;
/// A peer whose shares are mostly uncreditable after this many is disconnected.
const STALE_SHARE_LIMIT: u32 = 256;
/// Locator entries read from a peer (ours have about 30).
const MAX_LOCATOR: usize = 101;
/// Requests of one peer waiting to be served (one at a time, in order).
const MAX_SERVING_EACH: usize = 32;
/// Blocks kept while their parent is missing: in all, per peer, in bytes, and how long.
const MAX_ORPHANS: usize = 128;
const MAX_ORPHANS_EACH: usize = 32;
const MAX_ORPHAN_BYTES: usize = 64 * 1024 * 1024;
const ORPHAN_TTL: Duration = Duration::from_secs(20 * 60);
/// Blocks that were not stored, remembered so a peer replaying them costs nothing.
const MAX_DROPPED: usize = 8192;
/// A peer that sends this many blocks leading nowhere within ten minutes is disconnected.
const MAX_USELESS_BLOCKS: u32 = 500;
/// Transactions of one peer waiting for their proof to be checked; more are dropped.
const MAX_TX_CHECKS_EACH: usize = 32;
/// Transactions whose proof failed, remembered so the same bytes cost nothing again.
const MAX_REJECTED_TXS: usize = 8192;
/// Blocks one announcement may make this node ask for (new blocks come one or two at a
/// time; catching up goes through `GetHashes`).
const MAX_INV_BLOCKS: usize = 64;

/// Messages a peer may send per minute and in a burst; honest peers stay far below.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Ask {
    Hello,
    Hashes,
    Blocks,
    Addr,
    Shares,
    Txs,
    Inv,
}

impl Ask {
    fn budget(self) -> (f64, f64) {
        match self {
            Ask::Hello => (6.0, 3.0),
            Ask::Hashes => (120.0, 60.0),
            Ask::Blocks => (1200.0, 400.0),
            Ask::Addr => (6.0, 3.0),
            // The network finds about 32 shares a minute, each relayed on its own.
            Ask::Shares => (240.0, 60.0),
            // The whole network has sent fewer than a thousand transactions so far.
            Ask::Txs => (240.0, 120.0),
            Ask::Inv => (240.0, 120.0),
        }
    }
}

/// A peer's turn to be served, and how many of its requests wait for it.
type Turn = (Arc<tokio::sync::Mutex<()>>, usize);

/// Blocks waiting for their parent; they are tried again when it connects.
#[derive(Default)]
struct Orphans {
    blocks: HashMap<Hash32, (Block, PeerId, Instant, usize)>,
    by_prev: HashMap<Hash32, Vec<Hash32>>,
    bytes: usize,
}

impl Orphans {
    fn remove(&mut self, h: &Hash32) -> Option<(Block, PeerId)> {
        let (b, from, _, size) = self.blocks.remove(h)?;
        self.bytes -= size;
        if let Some(v) = self.by_prev.get_mut(&b.header.prev) {
            v.retain(|x| x != h);
            if v.is_empty() {
                self.by_prev.remove(&b.header.prev);
            }
        }
        Some((b, from))
    }

    /// Keeps `b` until its parent arrives; false when it was not kept.
    fn add(&mut self, b: Block, from: PeerId, size: usize) -> bool {
        let h = b.hash();
        if self.blocks.contains_key(&h) || size > MAX_ORPHAN_BYTES {
            return false;
        }
        let old: Vec<Hash32> = self
            .blocks
            .iter()
            .filter(|(_, x)| x.2.elapsed() > ORPHAN_TTL)
            .map(|(k, _)| *k)
            .collect();
        for k in old {
            self.remove(&k);
        }
        if self.blocks.values().filter(|x| x.1 == from).count() >= MAX_ORPHANS_EACH {
            return false;
        }
        while self.blocks.len() >= MAX_ORPHANS || self.bytes + size > MAX_ORPHAN_BYTES {
            let Some(oldest) = self.blocks.iter().min_by_key(|(_, x)| x.2).map(|(k, _)| *k) else {
                break;
            };
            self.remove(&oldest);
        }
        self.by_prev.entry(b.header.prev).or_default().push(h);
        self.blocks.insert(h, (b, from, Instant::now(), size));
        self.bytes += size;
        true
    }

    fn children(&mut self, parent: &Hash32) -> Vec<(Block, PeerId)> {
        let ids = self.by_prev.get(parent).cloned().unwrap_or_default();
        ids.iter().filter_map(|h| self.remove(h)).collect()
    }
}

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
    /// Mined on a block this node does not have on its main chain: the sender follows
    /// another branch, or is ahead of this node.
    OtherBranch,
    /// For a height above this node's next block: the sender is ahead of this node.
    Ahead,
    /// Fails the share target: the sender did no work.
    BadWork,
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareError::Stale(m) => write!(f, "share not creditable: {m}"),
            ShareError::OtherBranch => write!(f, "share not creditable: not on the main chain"),
            ShareError::Ahead => write!(f, "share not creditable: outside the inclusion window"),
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
    /// Held while the hashrate is computed (once per block).
    pub hashrate_calc: Mutex<()>,
    pub stats: Arc<Mutex<crate::stats::Stats>>,
    /// Latest `Stats::summary`, readable without waiting for a refresh.
    pub stats_summary: Arc<Mutex<serde_json::Value>>,
    pub recent_shares: Mutex<Option<RecentShares>>,
    pub events: broadcast::Sender<String>,
    pub jobs: Mutex<HashMap<u64, Job>>,
    pub templates: Mutex<HashMap<String, (Hash32, u64, Instant, u64)>>,
    pub template_budget: Mutex<Vec<Instant>>,
    /// Templates built for each client in the last minute.
    pub template_each: Mutex<HashMap<std::net::IpAddr, Vec<Instant>>>,
    /// New directory entries from each client in the last day.
    pub address_each: Mutex<HashMap<std::net::IpAddr, Vec<Instant>>>,
    /// Proof-of-work checks of submitted solutions running at once.
    pub pow_checks: tokio::sync::Semaphore,
    /// When each peer last sent us addresses (one list per peer every 10 minutes).
    pub addr_from: Mutex<HashMap<PeerId, Instant>>,
    /// Recent address-directory writes (bounds disk use from the public API).
    pub address_budget: Mutex<Vec<Instant>>,
    /// Per-client HTTP budgets and connection caps.
    pub guard: Arc<crate::guard::Guard>,
    /// At most a few HTTP-submitted transaction proofs are verified at once.
    pub tx_verify: tokio::sync::Semaphore,
    /// The same for transactions from peers (apart, so peers cannot crowd out wallets).
    peer_tx_verify: Arc<tokio::sync::Semaphore>,
    /// Per peer: its transactions waiting for their proof to be checked.
    tx_checks: Mutex<HashMap<PeerId, usize>>,
    rejected_txs: Mutex<(HashSet<Hash32>, VecDeque<Hash32>)>,
    pub nonce: u64,
    pub started: Instant,
    peer_heights: Mutex<HashMap<PeerId, u64>>,
    /// The tip each peer announced (its Hello).
    peer_best: Mutex<HashMap<PeerId, Hash32>>,
    /// Blocks requested and not yet received: who was asked and when.
    in_flight: Mutex<HashMap<Hash32, (PeerId, Instant)>>,
    /// When we last asked each peer for hashes; unsolicited `Hashes` are ignored.
    hash_requests: Mutex<HashMap<PeerId, Instant>>,
    /// Uncreditable shares per peer: (stale, total) since connecting.
    share_counts: Mutex<HashMap<PeerId, (u32, u32)>>,
    /// Our own transactions in the stem phase: (txid, first seen).
    stem: Mutex<Vec<(Hash32, Instant)>>,
    pub listen_port: u16,
    /// Swap offers, mailboxes and the market price (`/v1/swap/...`).
    pub board: crate::swapboard::Board,
    orphans: Mutex<Orphans>,
    /// Per peer: its requests' turn (one served at a time, in order) and how many wait.
    serving: Mutex<HashMap<PeerId, Turn>>,
    /// Per peer and kind of message: (tokens, last refill).
    asks: Mutex<HashMap<(PeerId, Ask), (f64, Instant)>>,
    /// Per peer: blocks leading nowhere in the current ten minutes.
    useless: Mutex<HashMap<PeerId, (u32, Instant)>>,
    dropped: Mutex<(HashSet<Hash32>, VecDeque<Hash32>)>,
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
            hashrate_calc: Mutex::new(()),
            stats: Arc::new(Mutex::new(crate::stats::Stats::default())),
            stats_summary: Arc::new(Mutex::new(serde_json::Value::Null)),
            recent_shares: Mutex::new(None),
            events,
            jobs: Mutex::new(HashMap::new()),
            templates: Mutex::new(HashMap::new()),
            template_budget: Mutex::new(Vec::new()),
            template_each: Mutex::new(HashMap::new()),
            address_each: Mutex::new(HashMap::new()),
            pow_checks: tokio::sync::Semaphore::new(
                std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1)),
            ),
            addr_from: Mutex::new(HashMap::new()),
            address_budget: Mutex::new(Vec::new()),
            guard: Arc::new(crate::guard::Guard::default()),
            tx_verify: tokio::sync::Semaphore::new(4),
            peer_tx_verify: Arc::new(tokio::sync::Semaphore::new(4)),
            tx_checks: Mutex::new(HashMap::new()),
            rejected_txs: Mutex::new((HashSet::new(), VecDeque::new())),
            nonce: rand::random(),
            started: Instant::now(),
            peer_heights: Mutex::new(HashMap::new()),
            peer_best: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            hash_requests: Mutex::new(HashMap::new()),
            share_counts: Mutex::new(HashMap::new()),
            stem: Mutex::new(Vec::new()),
            listen_port,
            board: Default::default(),
            orphans: Mutex::new(Orphans::default()),
            serving: Mutex::new(HashMap::new()),
            asks: Mutex::new(HashMap::new()),
            useless: Mutex::new(HashMap::new()),
            dropped: Mutex::new((HashSet::new(), VecDeque::new())),
        })
    }

    /// Takes one message of kind `a` from the peer's budget.
    fn allow(&self, p: PeerId, a: Ask) -> bool {
        let (per_min, burst) = a.budget();
        let now = Instant::now();
        let mut m = self.asks.lock();
        let e = m.entry((p, a)).or_insert((burst, now));
        e.0 = (e.0 + now.duration_since(e.1).as_secs_f64() * per_min / 60.0).min(burst);
        e.1 = now;
        if e.0 >= 1.0 {
            e.0 -= 1.0;
            true
        } else {
            false
        }
    }

    /// Runs a peer's request on the blocking pool, after its earlier ones: one peer's
    /// requests never hold up the messages of the others. Beyond a few waiting, dropped.
    fn serve(self: &Arc<Self>, p: PeerId, job: impl FnOnce(&Node) + Send + 'static) {
        let turn = {
            let mut m = self.serving.lock();
            let e = m
                .entry(p)
                .or_insert_with(|| (Arc::new(tokio::sync::Mutex::new(())), 0));
            if e.1 >= MAX_SERVING_EACH {
                return;
            }
            e.1 += 1;
            e.0.clone()
        };
        let me = self.clone();
        tokio::spawn(async move {
            let _turn = turn.lock().await;
            let n = me.clone();
            let _ = tokio::task::spawn_blocking(move || job(&n)).await;
            if let Some(e) = me.serving.lock().get_mut(&p) {
                e.1 = e.1.saturating_sub(1);
            }
        });
    }

    /// A block from `p` that led nowhere (known, not stored, no parent).
    fn useless(&self, p: PeerId) {
        let mut m = self.useless.lock();
        let e = m.entry(p).or_insert((0, Instant::now()));
        if e.1.elapsed() > Duration::from_secs(600) {
            *e = (0, Instant::now());
        }
        e.0 += 1;
        if e.0 > MAX_USELESS_BLOCKS {
            drop(m);
            tracing::warn!("peer {p} keeps sending blocks that lead nowhere");
            self.net.disconnect(p);
        }
    }

    fn was_dropped(&self, h: &Hash32) -> bool {
        self.dropped.lock().0.contains(h)
    }

    fn remember_rejected(&self, id: Hash32) {
        let mut d = self.rejected_txs.lock();
        if d.0.insert(id) {
            d.1.push_back(id);
            if d.1.len() > MAX_REJECTED_TXS
                && let Some(old) = d.1.pop_front()
            {
                d.0.remove(&old);
            }
        }
    }

    /// Checks a peer's transaction off the message loop (its proof takes milliseconds):
    /// at most a few at once from all peers and from each, the rest dropped.
    fn check_peer_tx(self: &Arc<Self>, p: PeerId, tx: Transaction) {
        {
            let mut m = self.tx_checks.lock();
            let n = m.entry(p).or_insert(0);
            if *n >= MAX_TX_CHECKS_EACH {
                return;
            }
            *n += 1;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let id = tx.txid();
            let permit = me.peer_tx_verify.clone().acquire_owned().await;
            let result = if me.net.connected(p) {
                Some(me.accept_tx(tx).await)
            } else {
                None
            };
            drop(permit);
            if let Some(m) = me.tx_checks.lock().get_mut(&p) {
                *m = m.saturating_sub(1);
            }
            match result {
                Some(Ok(id)) => {
                    me.net.note_tx(p);
                    me.net.broadcast(
                        &Message::Inv {
                            blocks: vec![],
                            txs: vec![id],
                        },
                        Some(p),
                    )
                }
                // Honest nodes only relay transactions whose proofs they verified.
                Some(Err(TxReject::Invalid(e))) => {
                    me.remember_rejected(id);
                    tracing::warn!("peer {p} sent an invalid transaction: {e}");
                    me.net.ban(p);
                }
                Some(Err(e)) => tracing::debug!("rejected tx from {p}: {e}"),
                None => {}
            }
        });
    }

    fn remember_dropped(&self, h: Hash32) {
        let mut d = self.dropped.lock();
        if d.0.insert(h) {
            d.1.push_back(h);
            if d.1.len() > MAX_DROPPED
                && let Some(old) = d.1.pop_front()
            {
                d.0.remove(&old);
            }
        }
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
            self.hash_requests.lock().insert(peer, Instant::now());
            self.net
                .send(peer, &Message::GetHashes { locator, max: 500 });
        }
    }

    /// Marks `h` as requested from `p` unless another request for it is still fresh.
    /// Bounded in total and per peer; entries expire so a peer that never delivers
    /// cannot stall the download.
    fn claim_block(&self, h: Hash32, p: PeerId) -> bool {
        let mut f = self.in_flight.lock();
        let now = Instant::now();
        if let Some((_, t)) = f.get(&h)
            && now.duration_since(*t) < IN_FLIGHT_TTL
        {
            return false;
        }
        if f.len() >= MAX_IN_FLIGHT {
            f.retain(|_, (_, t)| now.duration_since(*t) < IN_FLIGHT_TTL);
            if f.len() >= MAX_IN_FLIGHT {
                // Full: the peer holding the most requests gives up its older half, so
                // announcements that are never delivered cannot crowd out real blocks.
                let mut per: HashMap<PeerId, usize> = HashMap::new();
                for (q, _) in f.values() {
                    *per.entry(*q).or_default() += 1;
                }
                let Some((top, _)) = per.into_iter().max_by_key(|x| x.1) else {
                    return false;
                };
                if top == p {
                    return false;
                }
                let mut theirs: Vec<(Hash32, Instant)> = f
                    .iter()
                    .filter(|(_, (q, _))| *q == top)
                    .map(|(h, (_, t))| (*h, *t))
                    .collect();
                theirs.sort_by_key(|x| x.1);
                let n = theirs.len().div_ceil(2);
                for (h, _) in theirs.into_iter().take(n) {
                    f.remove(&h);
                }
            }
        }
        if f.values().filter(|(q, _)| *q == p).count() >= MAX_IN_FLIGHT_PER_PEER {
            return false;
        }
        f.insert(h, (p, now));
        true
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
                    self.peer_best.lock().remove(&p);
                    self.hash_requests.lock().remove(&p);
                    self.share_counts.lock().remove(&p);
                    self.addr_from.lock().remove(&p);
                    self.serving.lock().remove(&p);
                    self.asks.lock().retain(|(q, _), _| *q != p);
                    self.useless.lock().remove(&p);
                    self.tx_checks.lock().remove(&p);
                    // Whatever it was asked for may be asked from others right away.
                    self.in_flight.lock().retain(|_, (q, _)| *q != p);
                }
                // A peer dropped or banned since: what it queued before is not handled.
                Event::Message(p, _) if !self.net.connected(p) => {}
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
            self.in_flight
                .lock()
                .retain(|_, (_, t)| t.elapsed() < IN_FLIGHT_TTL);
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
                if !self.allow(p, Ask::Hello) {
                    self.net.disconnect(p);
                    return Ok(());
                }
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
                // A claimed height can't exceed what the time since genesis allows
                // (blocks every 60 s on average; twice as fast is already implausible).
                let elapsed = krypton_core::now().saturating_sub(self.chain.params.genesis_time);
                let plausible = elapsed / (krypton_core::difficulty::TARGET_SPACING / 2) + 1_000;
                self.peer_heights.lock().insert(p, h.height.min(plausible));
                self.peer_best.lock().insert(p, h.best);
                self.net.set_hello(p, h.clone());
                let tip = self.chain.tip();
                if h.height > tip.height() {
                    self.request_hashes(p);
                }
                self.net.send(p, &Message::GetAddr);
                // Our pending shares only help a peer on our chain near our tip; to one on
                // another branch or far behind they are uncreditable, and nodes before 1.0.53
                // disconnect whoever sends many of those (which kept a partition from healing).
                let same_chain = h.best == tip.hash
                    || (h.height + SHARE_WINDOW > tip.height()
                        && self.chain.is_main(&h.best).unwrap_or(false));
                let pending: Vec<Vec<u8>> = if same_chain {
                    self.shares
                        .lock()
                        .recent(krypton_p2p::message::MAX_SHARES_PER_MESSAGE)
                        .iter()
                        .map(|s| s.to_bytes())
                        .collect()
                } else {
                    vec![]
                };
                if !pending.is_empty() {
                    self.net.send(p, &Message::Shares(pending));
                }
            }
            Message::Shares(v) => {
                if !self.allow(p, Ask::Shares) {
                    return Ok(());
                }
                // Shares of another branch, or from a peer ahead of us, are expected from a
                // peer that is not (yet) on our chain: only a peer that claims our chain and
                // still sends them is abusing.
                let ours = self
                    .peer_best
                    .lock()
                    .get(&p)
                    .is_some_and(|b| self.chain.is_main(b).unwrap_or(false));
                let ahead = self.peer_heights.lock().get(&p).copied().unwrap_or(0)
                    > self.chain.tip().height();
                let total = v.len();
                let mut stale = 0usize;
                for raw in v {
                    let Ok(s) = BlockHeader::from_bytes(&raw) else {
                        self.net.ban(p);
                        return Ok(());
                    };
                    match self.accept_share(s, Some(p), false).await {
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
                        Err(e @ (ShareError::OtherBranch | ShareError::Ahead)) => {
                            let expected = match e {
                                ShareError::OtherBranch => !ours,
                                _ => ahead,
                            };
                            if !expected {
                                stale += 1;
                            }
                            tracing::debug!("share from {p}: {e}");
                        }
                    }
                }
                // Honest peers send recent shares; a flood of uncreditable ones is abuse,
                // counted across messages so it can't be split into small batches.
                let (s_all, t_all) = {
                    let mut c = self.share_counts.lock();
                    let e = c.entry(p).or_insert((0, 0));
                    e.0 = e.0.saturating_add(stale as u32);
                    e.1 = e.1.saturating_add(total as u32);
                    *e
                };
                if (total >= 64 && stale * 2 > total)
                    || (s_all >= STALE_SHARE_LIMIT && s_all * 2 > t_all)
                {
                    tracing::warn!("peer {p} sent {s_all}/{t_all} uncreditable shares");
                    self.net.disconnect(p);
                }
            }
            Message::Ping(n) => self.net.send(p, &Message::Pong(n)),
            Message::Pong(_) => {}
            Message::GetHashes { mut locator, max } => {
                if !self.allow(p, Ask::Hashes) {
                    return Ok(());
                }
                locator.truncate(MAX_LOCATOR);
                let max = (max as usize).min(2000);
                self.serve(p, move |n| match n.chain.hashes_after(&locator, max) {
                    Ok(v) => n.net.send(p, &Message::Hashes(v)),
                    Err(e) => tracing::error!("storage error: {e}"),
                });
            }
            Message::Hashes(v) => {
                let asked = self
                    .hash_requests
                    .lock()
                    .get(&p)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(120));
                if !asked {
                    return Ok(());
                }
                let mut want = Vec::new();
                for h in v.into_iter().take(2000) {
                    if !self.chain.has_block(&h)? && self.claim_block(h, p) {
                        want.push(h);
                    }
                }
                for c in want.chunks(32) {
                    self.net.send(p, &Message::GetBlocks(c.to_vec()));
                }
            }
            Message::GetBlocks(v) => {
                if !self.allow(p, Ask::Blocks) {
                    return Ok(());
                }
                // Requests beyond what the peer is reading are dropped; it asks again.
                self.serve(p, move |n| {
                    for h in v.iter().take(64) {
                        if n.net.queued_bytes(p) > krypton_p2p::SOFT_QUEUED_BYTES {
                            break;
                        }
                        match n.chain.block(h) {
                            Ok(Some(b)) => n.net.send(p, &Message::Block(b.to_bytes())),
                            Ok(None) => {}
                            Err(e) => {
                                tracing::error!("storage error: {e}");
                                break;
                            }
                        }
                    }
                });
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
                if self.was_dropped(&hash) {
                    self.useless(p);
                    return Ok(());
                }
                self.block_from(p, block, raw.len()).await;
            }
            Message::Inv { blocks, txs } => {
                if !self.allow(p, Ask::Inv) {
                    return Ok(());
                }
                let mut want_b = Vec::new();
                for h in blocks.into_iter().take(MAX_INV_BLOCKS) {
                    if !self.chain.has_block(&h)? && self.claim_block(h, p) {
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
                if !self.allow(p, Ask::Txs) {
                    return Ok(());
                }
                let Ok(tx) = Transaction::from_bytes(&raw) else {
                    self.net.ban(p);
                    return Ok(());
                };
                // The same invalid bytes again: known already, no proof to check.
                if self.rejected_txs.lock().0.contains(&tx.txid()) {
                    self.net.ban(p);
                    return Ok(());
                }
                self.check_peer_tx(p, tx);
            }
            Message::GetAddr => {
                if self.allow(p, Ask::Addr) {
                    self.net.send(p, &Message::Addr(self.net.addrs()))
                }
            }
            Message::Addr(v) => {
                let mut seen = self.addr_from.lock();
                let fresh = seen
                    .get(&p)
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(600));
                if fresh {
                    seen.insert(p, Instant::now());
                    drop(seen);
                    self.net.add_addrs(v);
                }
            }
        }
        Ok(())
    }

    /// A block from peer `p`, and then any waiting blocks it is the parent of.
    async fn block_from(self: &Arc<Self>, p: PeerId, block: Block, size: usize) {
        let mut queue = vec![(block, p, size)];
        while let Some((block, from, size)) = queue.pop() {
            let hash = block.hash();
            let height = block.header.height;
            let ready = |q: &mut Vec<(Block, PeerId, usize)>| {
                for (b, f) in self.orphans.lock().children(&hash) {
                    let n = b.to_bytes().len();
                    q.push((b, f, n));
                }
            };
            match self.submit_block(block.clone()).await {
                Ok(Outcome::Connected { .. }) => {
                    self.net.note_block(from);
                    let mut ph = self.peer_heights.lock();
                    let e = ph.entry(from).or_insert(0);
                    *e = (*e).max(height);
                    drop(ph);
                    ready(&mut queue);
                    if queue.is_empty()
                        && self.in_flight.lock().is_empty()
                        && self.best_peer_height() > self.chain.tip().height()
                    {
                        self.request_hashes(from);
                    }
                }
                Ok(Outcome::Orphan { .. }) => {
                    if !self.orphans.lock().add(block, from, size) {
                        self.useless(from);
                    }
                    // Ask for what leads to it, at most every few seconds.
                    let recent = self
                        .hash_requests
                        .lock()
                        .get(&from)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(5));
                    if !recent {
                        self.request_hashes(from);
                    }
                }
                Ok(Outcome::SideChain) => {
                    if self.chain.has_block(&hash).unwrap_or(false) {
                        // A branch with real work, kept: what builds on it may follow.
                        ready(&mut queue);
                    } else {
                        self.remember_dropped(hash);
                        self.useless(from);
                    }
                }
                Ok(Outcome::AlreadyKnown) => self.useless(from),
                Err(ChainError::Invalid(CoreError::Future)) => {}
                Err(ChainError::Invalid(e)) => {
                    tracing::warn!("invalid block from peer {from}: {e}");
                    self.net.ban(from);
                }
                Err(e) => tracing::error!("storage error: {e}"),
            }
        }
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
        work_checked: bool,
    ) -> std::result::Result<ShareOutcome, ShareError> {
        let hash = s.hash();
        if self.shares.lock().shares.contains_key(&hash) {
            return Ok(ShareOutcome::Known);
        }
        // Cheap window check before any storage access.
        let next = self.chain.tip().height() + 1;
        if s.height > next {
            return Err(ShareError::Ahead);
        }
        if s.height == 0 || s.height + SHARE_WINDOW <= next {
            return Err(ShareError::Stale("outside the inclusion window".into()));
        }
        // Creditable shares extend the main chain: one lookup rejects the rest cheaply.
        if self.chain.main_hash(s.height - 1).ok().flatten() != Some(s.prev) {
            return Err(ShareError::OtherBranch);
        }
        let chain = self.chain.clone();
        let s2 = s.clone();
        tokio::task::spawn_blocking(move || {
            if !chain.share_includable(&s2) {
                return Err(ShareError::Stale(
                    "outside the window, already credited or not on the main chain".into(),
                ));
            }
            if work_checked {
                return Ok(());
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
        // Token transactions before their activation are not merely early: no honest
        // wallet sends them, so whoever relays one is treated like any sender of junk.
        if tx.asset.is_some() && self.chain.tip().height() + 1 < self.chain.params.assets_height {
            return Err(TxReject::Invalid(
                "tokens are not active on this network".into(),
            ));
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
