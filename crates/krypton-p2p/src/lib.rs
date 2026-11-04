//! Peer-to-peer networking: encrypted sessions, connection limits, address book.
//! Protocol logic (sync, relay) lives in the node, which consumes [`Event`]s.

#![forbid(unsafe_code)]

pub mod message;
pub mod session;

pub use message::{Hello, Message};

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};

/// A peer whose unsent data exceeds this is not reading: it is disconnected.
pub const MAX_QUEUED_BYTES: usize = 128 * 1024 * 1024;
/// Serving bulk data (blocks, transactions) pauses while a peer has this much unsent.
pub const SOFT_QUEUED_BYTES: usize = 32 * 1024 * 1024;
/// Inbound connections still in the handshake, in total.
const MAX_PENDING_HANDSHAKES: usize = 64;

pub type PeerId = u64;

#[derive(Clone, Debug)]
pub struct Config {
    pub magic: [u8; 4],
    pub listen: Option<SocketAddr>,
    pub seeds: Vec<String>,
    pub max_outbound: usize,
    pub max_inbound: usize,
    pub max_per_ip: usize,
}

#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub id: PeerId,
    pub addr: SocketAddr,
    pub inbound: bool,
    pub hello: Option<Hello>,
    pub since: u64,
}

#[derive(Debug)]
pub enum Event {
    Connected(PeerId),
    Message(PeerId, Message),
    Disconnected(PeerId),
}

struct Peer {
    info: PeerInfo,
    tx: mpsc::Sender<Vec<u8>>,
    dial: Option<String>,
    /// Bytes handed to the writer task and not yet written to the socket.
    queued: Arc<AtomicUsize>,
}

impl Peer {
    /// Queues a frame; false when the peer is too far behind and must be dropped.
    fn enqueue(&self, bytes: Vec<u8>) -> bool {
        let n = bytes.len();
        if self.queued.load(Ordering::Relaxed) + n > MAX_QUEUED_BYTES {
            return false;
        }
        self.queued.fetch_add(n, Ordering::Relaxed);
        if self.tx.try_send(bytes).is_err() {
            // Silently dropping a frame (a block reply, say) would stall the peer's sync.
            self.queued.fetch_sub(n, Ordering::Relaxed);
            return false;
        }
        true
    }
}

/// Limits and bans apply per IPv4 address and per IPv6 /64 (one customer's prefix).
fn ip_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// An inbound peer must introduce itself (Hello) this soon or lose its slot.
const HELLO_DEADLINE: Duration = Duration::from_secs(30);
const MAX_ADDR_BOOK: usize = 5000;

struct Inner {
    cfg: Config,
    peers: Mutex<HashMap<PeerId, Peer>>,
    next: AtomicU64,
    events: mpsc::Sender<Event>,
    addrs: Mutex<HashSet<String>>,
    banned: Mutex<HashMap<IpAddr, Instant>>,
    handshakes: Arc<Semaphore>,
    /// Inbound handshakes in progress per IP.
    pending: Mutex<HashMap<IpAddr, usize>>,
    /// Outbound targets being dialed right now.
    dialing: Mutex<HashSet<String>>,
}

#[derive(Clone)]
pub struct Net {
    inner: Arc<Inner>,
}

const BAN_TIME: Duration = Duration::from_secs(24 * 3600);

impl Net {
    pub fn start(cfg: Config) -> (Net, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel(4096);
        let addrs = cfg.seeds.iter().cloned().collect();
        let net = Net {
            inner: Arc::new(Inner {
                cfg,
                peers: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                events: tx,
                addrs: Mutex::new(addrs),
                banned: Mutex::new(HashMap::new()),
                handshakes: Arc::new(Semaphore::new(MAX_PENDING_HANDSHAKES)),
                pending: Mutex::new(HashMap::new()),
                dialing: Mutex::new(HashSet::new()),
            }),
        };
        if let Some(l) = net.inner.cfg.listen {
            let n = net.clone();
            tokio::spawn(async move {
                if let Err(e) = n.listen(l).await {
                    tracing::error!("p2p listener stopped: {e}");
                }
            });
        }
        let n = net.clone();
        tokio::spawn(async move { n.dial_loop().await });
        (net, rx)
    }

    async fn listen(&self, addr: SocketAddr) -> anyhow::Result<()> {
        let l = TcpListener::bind(addr).await?;
        tracing::info!("p2p listening on {addr}");
        loop {
            let (s, peer) = match l.accept().await {
                Ok(x) => x,
                Err(e) => {
                    // Transient (e.g. out of file descriptors): keep listening.
                    tracing::warn!("p2p accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            if self.is_banned(&peer.ip()) {
                continue;
            }
            let key = ip_key(peer.ip());
            let (inbound, same_ip) = {
                let p = self.inner.peers.lock();
                (
                    p.values().filter(|x| x.info.inbound).count(),
                    p.values().filter(|x| ip_key(x.info.addr.ip()) == key).count(),
                )
            };
            if inbound >= self.inner.cfg.max_inbound || same_ip >= self.inner.cfg.max_per_ip {
                continue;
            }
            let Ok(permit) = self.inner.handshakes.clone().try_acquire_owned() else {
                continue;
            };
            {
                let mut p = self.inner.pending.lock();
                let c = p.entry(key).or_insert(0);
                if *c >= self.inner.cfg.max_per_ip {
                    continue;
                }
                *c += 1;
            }
            let n = self.clone();
            tokio::spawn(async move {
                let magic = n.inner.cfg.magic;
                let hs =
                    tokio::time::timeout(Duration::from_secs(10), session::respond(s, magic)).await;
                {
                    let mut p = n.inner.pending.lock();
                    if let Some(c) = p.get_mut(&key) {
                        *c -= 1;
                        if *c == 0 {
                            p.remove(&key);
                        }
                    }
                }
                drop(permit);
                match hs {
                    Ok(Ok((send, recv))) => n.run_peer(peer, true, None, send, recv).await,
                    _ => tracing::debug!("inbound handshake with {peer} failed"),
                }
            });
        }
    }

    async fn dial_loop(&self) {
        loop {
            let (outbound, connected): (usize, HashSet<String>) = {
                let p = self.inner.peers.lock();
                (
                    p.values().filter(|x| !x.info.inbound).count(),
                    p.values().filter_map(|x| x.dial.clone()).collect(),
                )
            };
            let dialing = self.inner.dialing.lock().len();
            let free = self
                .inner
                .cfg
                .max_outbound
                .saturating_sub(outbound + dialing);
            if free > 0 {
                use rand::seq::SliceRandom;
                let busy =
                    |a: &String| connected.contains(a) || self.inner.dialing.lock().contains(a);
                let seeds = &self.inner.cfg.seeds;
                let mut first: Vec<String> = seeds.iter().filter(|a| !busy(a)).cloned().collect();
                let mut rest: Vec<String> = self
                    .inner
                    .addrs
                    .lock()
                    .iter()
                    .filter(|a| !seeds.contains(a))
                    .cloned()
                    .collect();
                rest.retain(|a| !busy(a));
                first.shuffle(&mut rand::thread_rng());
                rest.shuffle(&mut rand::thread_rng());
                // With few peers the seeds go first, so an address book flooded with
                // unreachable or hostile entries cannot keep the node isolated.
                if outbound >= 2 {
                    std::mem::swap(&mut first, &mut rest);
                }
                for a in first.into_iter().chain(rest).take(free) {
                    let n = self.clone();
                    tokio::spawn(async move { n.dial(a).await });
                }
            }
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
    }

    pub async fn dial(&self, target: String) {
        if !self.inner.dialing.lock().insert(target.clone()) {
            return;
        }
        let session = self.handshake_with(&target).await;
        self.inner.dialing.lock().remove(&target);
        match session {
            Some((addr, send, recv)) => self.run_peer(addr, false, Some(target), send, recv).await,
            // Unreachable addresses are forgotten (seeds are always kept).
            None if !self.inner.cfg.seeds.contains(&target) => {
                self.inner.addrs.lock().remove(&target);
            }
            None => {}
        }
    }

    async fn handshake_with(
        &self,
        target: &str,
    ) -> Option<(SocketAddr, session::SendHalf, session::RecvHalf)> {
        let Ok(Ok(mut addrs)) =
            tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host(target)).await
        else {
            return None;
        };
        let addr = addrs.next()?;
        if self.is_banned(&addr.ip()) || Some(addr) == self.inner.cfg.listen {
            return None;
        }
        let Ok(Ok(s)) =
            tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr)).await
        else {
            return None;
        };
        let _ = s.set_nodelay(true);
        match tokio::time::timeout(
            Duration::from_secs(10),
            session::initiate(s, self.inner.cfg.magic),
        )
        .await
        {
            Ok(Ok((send, recv))) => Some((addr, send, recv)),
            _ => {
                tracing::debug!("outbound handshake with {addr} failed");
                None
            }
        }
    }

    async fn run_peer(
        &self,
        addr: SocketAddr,
        inbound: bool,
        dial: Option<String>,
        mut send: session::SendHalf,
        mut recv: session::RecvHalf,
    ) {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1024);
        let info = PeerInfo {
            id,
            addr,
            inbound,
            hello: None,
            since: krypton_core::now(),
        };
        let queued = Arc::new(AtomicUsize::new(0));
        self.inner.peers.lock().insert(
            id,
            Peer {
                info,
                tx,
                dial,
                queued: queued.clone(),
            },
        );
        let _ = self.inner.events.send(Event::Connected(id)).await;
        // Signals the reader once the peer is dropped or its socket stops accepting data.
        let closed = Arc::new(tokio::sync::Notify::new());
        let closed_w = closed.clone();
        let writer = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                let sent = send.send(&m).await;
                queued.fetch_sub(m.len(), Ordering::Relaxed);
                if sent.is_err() {
                    break;
                }
            }
            closed_w.notify_one();
        });
        let started = Instant::now();
        loop {
            let introduced = self
                .inner
                .peers
                .lock()
                .get(&id)
                .is_some_and(|p| p.info.hello.is_some());
            let wait = if inbound && !introduced {
                let left = HELLO_DEADLINE.saturating_sub(started.elapsed());
                if left.is_zero() {
                    tracing::debug!("inbound peer {addr} never introduced itself");
                    break;
                }
                left
            } else {
                Duration::from_secs(600)
            };
            let frame = tokio::select! {
                f = tokio::time::timeout(wait, recv.recv()) => f,
                _ = closed.notified() => break,
            };
            if frame.is_err() && inbound && !introduced {
                continue;
            }
            match frame {
                Ok(Ok(frame)) => match Message::decode(&frame) {
                    Ok(m) => {
                        if self.inner.events.send(Event::Message(id, m)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("peer {addr} sent a malformed message: {e}");
                        self.ban(id);
                        break;
                    }
                },
                _ => break,
            }
            if !self.inner.peers.lock().contains_key(&id) {
                break;
            }
        }
        writer.abort();
        self.inner.peers.lock().remove(&id);
        let _ = self.inner.events.send(Event::Disconnected(id)).await;
    }

    pub fn send(&self, peer: PeerId, m: &Message) {
        let mut ps = self.inner.peers.lock();
        if let Some(p) = ps.get(&peer)
            && !p.enqueue(m.encode())
        {
            tracing::debug!("peer {} stopped reading; disconnecting", p.info.addr);
            ps.remove(&peer);
        }
    }

    pub fn broadcast(&self, m: &Message, except: Option<PeerId>) {
        let bytes = m.encode();
        let mut ps = self.inner.peers.lock();
        let stalled: Vec<PeerId> = ps
            .iter()
            .filter(|(id, p)| Some(**id) != except && p.info.hello.is_some())
            .filter(|(_, p)| !p.enqueue(bytes.clone()))
            .map(|(id, _)| *id)
            .collect();
        for id in stalled {
            ps.remove(&id);
        }
    }

    /// Bytes queued for `peer` and not yet written (0 for unknown peers).
    pub fn queued_bytes(&self, peer: PeerId) -> usize {
        self.inner
            .peers
            .lock()
            .get(&peer)
            .map(|p| p.queued.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn set_hello(&self, peer: PeerId, h: Hello) {
        let mut ps = self.inner.peers.lock();
        if let Some(p) = ps.get_mut(&peer) {
            // Only a peer's first Hello may add its listening address to the book.
            if p.info.hello.is_none() && p.info.inbound && h.listen_port != 0 {
                let a = SocketAddr::new(p.info.addr.ip(), h.listen_port).to_string();
                let mut book = self.inner.addrs.lock();
                if book.len() < MAX_ADDR_BOOK {
                    book.insert(a);
                }
            }
            p.info.hello = Some(h);
        }
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        self.inner
            .peers
            .lock()
            .values()
            .map(|p| p.info.clone())
            .collect()
    }

    pub fn ready_peers(&self) -> Vec<PeerId> {
        self.inner
            .peers
            .lock()
            .values()
            .filter(|p| p.info.hello.is_some())
            .map(|p| p.info.id)
            .collect()
    }

    pub fn disconnect(&self, peer: PeerId) {
        self.inner.peers.lock().remove(&peer);
    }

    /// Disconnects and refuses the peer's IP for a day (invalid data, protocol abuse).
    pub fn ban(&self, peer: PeerId) {
        if let Some(p) = self.inner.peers.lock().remove(&peer) {
            if !p.info.addr.ip().is_loopback() {
                self.inner
                    .banned
                    .lock()
                    .insert(ip_key(p.info.addr.ip()), Instant::now());
            }
            tracing::warn!("banned peer {}", p.info.addr);
        }
    }

    fn is_banned(&self, ip: &IpAddr) -> bool {
        let mut b = self.inner.banned.lock();
        b.retain(|_, t| t.elapsed() < BAN_TIME);
        b.contains_key(&ip_key(*ip))
    }

    pub fn add_addrs(&self, v: Vec<String>) {
        let mut a = self.inner.addrs.lock();
        for x in v.into_iter().take(message::MAX_ADDRS) {
            if a.len() >= MAX_ADDR_BOOK {
                break;
            }
            if x.parse::<SocketAddr>()
                .is_ok_and(|s| !s.ip().is_unspecified() && s.port() != 0)
            {
                a.insert(x);
            }
        }
    }

    pub fn addrs(&self) -> Vec<String> {
        self.inner
            .addrs
            .lock()
            .iter()
            .take(message::MAX_ADDRS)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn two_nodes_handshake_and_exchange() {
        let la: SocketAddr = "127.0.0.1:38711".parse().unwrap();
        let (a, mut ea) = Net::start(Config {
            magic: *b"TEST",
            listen: Some(la),
            seeds: vec![],
            max_outbound: 8,
            max_inbound: 8,
            max_per_ip: 8,
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (b, mut eb) = Net::start(Config {
            magic: *b"TEST",
            listen: None,
            seeds: vec![],
            max_outbound: 8,
            max_inbound: 8,
            max_per_ip: 8,
        });
        let b2 = b.clone();
        tokio::spawn(async move { b2.dial(la.to_string()).await });
        let pa = loop {
            if let Some(Event::Connected(id)) = ea.recv().await {
                break id;
            }
        };
        let pb = loop {
            if let Some(Event::Connected(id)) = eb.recv().await {
                break id;
            }
        };
        a.send(pa, &Message::Ping(42));
        loop {
            if let Some(Event::Message(id, Message::Ping(42))) = eb.recv().await {
                assert_eq!(id, pb);
                break;
            }
        }
        // Wrong network magic is refused.
        let (c, _ec) = Net::start(Config {
            magic: *b"XXXX",
            listen: None,
            seeds: vec![],
            max_outbound: 8,
            max_inbound: 8,
            max_per_ip: 8,
        });
        c.dial(la.to_string()).await;
        assert!(c.peers().is_empty());
    }
}
