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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

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
}

struct Inner {
    cfg: Config,
    peers: Mutex<HashMap<PeerId, Peer>>,
    next: AtomicU64,
    events: mpsc::Sender<Event>,
    addrs: Mutex<HashSet<String>>,
    banned: Mutex<HashMap<IpAddr, Instant>>,
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
            let (s, peer) = l.accept().await?;
            if self.is_banned(&peer.ip()) {
                continue;
            }
            let (inbound, same_ip) = {
                let p = self.inner.peers.lock();
                (
                    p.values().filter(|x| x.info.inbound).count(),
                    p.values().filter(|x| x.info.addr.ip() == peer.ip()).count(),
                )
            };
            if inbound >= self.inner.cfg.max_inbound || same_ip >= self.inner.cfg.max_per_ip {
                continue;
            }
            let n = self.clone();
            tokio::spawn(async move {
                let magic = n.inner.cfg.magic;
                match tokio::time::timeout(Duration::from_secs(10), session::respond(s, magic))
                    .await
                {
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
            if outbound < self.inner.cfg.max_outbound {
                let mut candidates: Vec<String> = self
                    .inner
                    .addrs
                    .lock()
                    .iter()
                    .filter(|a| !connected.contains(*a))
                    .cloned()
                    .collect();
                use rand::seq::SliceRandom;
                candidates.shuffle(&mut rand::thread_rng());
                for a in candidates
                    .into_iter()
                    .take(self.inner.cfg.max_outbound - outbound)
                {
                    let n = self.clone();
                    tokio::spawn(async move { n.dial(a).await });
                }
            }
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
    }

    pub async fn dial(&self, target: String) {
        let Ok(Ok(mut addrs)) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::lookup_host(target.clone()),
        )
        .await
        else {
            return;
        };
        let Some(addr) = addrs.next() else { return };
        if self.is_banned(&addr.ip()) || Some(addr) == self.inner.cfg.listen {
            return;
        }
        let Ok(Ok(s)) =
            tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr)).await
        else {
            return;
        };
        let _ = s.set_nodelay(true);
        match tokio::time::timeout(
            Duration::from_secs(10),
            session::initiate(s, self.inner.cfg.magic),
        )
        .await
        {
            Ok(Ok((send, recv))) => self.run_peer(addr, false, Some(target), send, recv).await,
            _ => tracing::debug!("outbound handshake with {addr} failed"),
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
        self.inner.peers.lock().insert(id, Peer { info, tx, dial });
        let _ = self.inner.events.send(Event::Connected(id)).await;
        let writer = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if send.send(&m).await.is_err() {
                    break;
                }
            }
        });
        loop {
            match tokio::time::timeout(Duration::from_secs(600), recv.recv()).await {
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
        if let Some(p) = self.inner.peers.lock().get(&peer) {
            let _ = p.tx.try_send(m.encode());
        }
    }

    pub fn broadcast(&self, m: &Message, except: Option<PeerId>) {
        let bytes = m.encode();
        for (id, p) in self.inner.peers.lock().iter() {
            if Some(*id) != except && p.info.hello.is_some() {
                let _ = p.tx.try_send(bytes.clone());
            }
        }
    }

    pub fn set_hello(&self, peer: PeerId, h: Hello) {
        let mut ps = self.inner.peers.lock();
        if let Some(p) = ps.get_mut(&peer) {
            if p.info.inbound && h.listen_port != 0 {
                let a = SocketAddr::new(p.info.addr.ip(), h.listen_port).to_string();
                self.inner.addrs.lock().insert(a);
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
                    .insert(p.info.addr.ip(), Instant::now());
            }
            tracing::warn!("banned peer {}", p.info.addr);
        }
    }

    fn is_banned(&self, ip: &IpAddr) -> bool {
        let mut b = self.inner.banned.lock();
        b.retain(|_, t| t.elapsed() < BAN_TIME);
        b.contains_key(ip)
    }

    pub fn add_addrs(&self, v: Vec<String>) {
        let mut a = self.inner.addrs.lock();
        for x in v.into_iter().take(message::MAX_ADDRS) {
            if a.len() >= 5000 {
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
