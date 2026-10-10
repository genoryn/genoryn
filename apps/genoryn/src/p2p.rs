//! Bounded peer protocol over clearnet TCP and Tor. No discovery peer is trusted
//! for consensus. Every outbound connection is opened by `net::Connector`.
use crate::{
    net::{Link, NetClass, PeerAddr, PeerStream, WireAddr, addr::normalized_ip, tcp_stream},
    node::{BlockOutcome, Event, Node, Peer},
    relay::{self, Candidate, RelayQueue, RelayScope},
};
use anyhow::{Context, Result, ensure};
use genoryn_core::{
    block::Block,
    chain::{is_policy_rejection, is_unknown_parent},
    codec::{self, Hash},
    params::Network,
    privacy::ShieldedTx,
};
use rand08::{Rng, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
};

pub const MAX_FRAME: usize = 2_100_000;
const MAX_KNOWN: usize = 128;
/// Onion identities cost nothing to create, so they may fill at most half the cache.
const MAX_KNOWN_ONION: usize = 64;
/// Discovery entries per IPv4 /16 or IPv6 /32, so one operator cannot fill the cache.
const MAX_PER_NETGROUP: usize = 4;
/// Outbound connections per network group, so the peers this node chooses are diverse.
const MAX_OUTBOUND_PER_NETGROUP: usize = 2;
/// Inbound connections per remote IP; loopback labs are exempt.
const MAX_INBOUND_PER_IP: usize = 4;
/// Onion-service connections all arrive from Tor on loopback and cannot be told
/// apart or banned by address, so their arrival rate is bounded instead.
const MAX_ONION_INBOUND_PER_MINUTE: u32 = 30;
const MAX_BANS: usize = 4096;
const BAN_SECONDS: u64 = 300;
const ROTATION_MIN_SECONDS: u64 = 15 * 60;
const ROTATION_MAX_SECONDS: u64 = 30 * 60;
/// Consecutive failed dials after which a discovered (never an explicit) address is forgotten.
const FORGET_AFTER_FAILURES: u8 = 5;
const MISBEHAVIOR_LIMIT: u32 = 100;
const INVALID_BLOCK_STRIKES: u32 = 20;
const INVALID_TRANSACTION_STRIKES: u32 = 10;
/// A side block whose branch trails the tip by more than this many blocks of
/// current-difficulty work is cheap historical spam. It is still validated and
/// stored (fork choice is unchanged); its sender only earns strikes.
const LOW_WORK_WINDOW_BLOCKS: u128 = 720;
const LOW_WORK_STRIKES: u32 = 2;
const REQUEST_WINDOW: Duration = Duration::from_secs(10);
const MAX_GET_BLOCKS_PER_WINDOW: u32 = 300;
const MAX_PINGS_PER_WINDOW: u32 = 20;
const PEER_LIST_WINDOW: Duration = Duration::from_secs(600);
const MAX_PEER_LISTS_PER_WINDOW: u32 = 3;
const MAX_ADVERTISED: usize = 32;
/// Feature announcements ride in an unsolicited `Pong`, which every v1 node
/// ignores, so announcing never disturbs an unupgraded peer. The high half of
/// the nonce is "GNRF"; the low half carries feature bits.
const FEATURE_MAGIC: u64 = 0x474E_5246_0000_0000;
const FEATURE_ADDR_V2: u64 = 1;
const OUR_FEATURES: u64 = FEATURE_ADDR_V2;

/// Fixed-window request counter for one connection.
struct RequestWindow {
    start: Instant,
    count: u32,
}
impl RequestWindow {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            count: 0,
        }
    }
    /// Counts one request; false once more than `limit` arrive within `period`.
    fn allow(&mut self, now: Instant, period: Duration, limit: u32) -> bool {
        if now.saturating_duration_since(self.start) >= period {
            self.start = now;
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count <= limit
    }
}
/// Policy refusals cost nothing, a block we cannot connect yet costs little (its
/// sender may be ahead of us) and invalid data is severe.
fn block_penalty(error: &anyhow::Error) -> u32 {
    if is_policy_rejection(error) {
        0
    } else if is_unknown_parent(error) {
        1
    } else {
        INVALID_BLOCK_STRIKES
    }
}
fn transaction_penalty(error: &anyhow::Error) -> u32 {
    if is_policy_rejection(error) {
        0
    } else {
        INVALID_TRANSACTION_STRIKES
    }
}
fn low_work_side_block(outcome: &BlockOutcome) -> bool {
    let accepted = &outcome.acceptance;
    !accepted.became_best
        && accepted.branch_work.is_some_and(|work| {
            outcome.tip_work.saturating_sub(work)
                > LOW_WORK_WINDOW_BLOCKS.saturating_mul(u128::from(outcome.next_difficulty))
        })
}
fn netgroup(address: &PeerAddr) -> Option<(u8, u32)> {
    match address {
        PeerAddr::Ip(socket) => match normalized_ip(socket.ip()) {
            ip if ip.is_loopback() => None,
            IpAddr::V4(ip) => {
                let o = ip.octets();
                Some((4, u32::from_be_bytes([o[0], o[1], 0, 0])))
            }
            IpAddr::V6(ip) => {
                let o = ip.octets();
                Some((6, u32::from_be_bytes([o[0], o[1], o[2], o[3]])))
            }
        },
        PeerAddr::Onion { key, .. } => Some((0x0e, u32::from(key.public_key()[0]))),
        PeerAddr::Name { .. } => None,
    }
}
fn record_ban(bans: &mut BTreeMap<IpAddr, Instant>, ip: IpAddr, now: Instant) {
    bans.retain(|_, until| *until > now);
    if bans.len() < MAX_BANS {
        bans.insert(ip, now + Duration::from_secs(BAN_SECONDS));
    }
}
/// Holds one of a remote IP's inbound connection slots until dropped.
struct InboundIpSlot {
    slots: Arc<Mutex<BTreeMap<IpAddr, usize>>>,
    ip: IpAddr,
}
impl InboundIpSlot {
    fn acquire(slots: &Arc<Mutex<BTreeMap<IpAddr, usize>>>, ip: IpAddr) -> Option<Self> {
        let mut map = slots.lock().ok()?;
        let count = map.get(&ip).copied().unwrap_or(0);
        if !ip.is_loopback() && count >= MAX_INBOUND_PER_IP {
            return None;
        }
        map.insert(ip, count + 1);
        Some(Self {
            slots: slots.clone(),
            ip,
        })
    }
}
impl Drop for InboundIpSlot {
    fn drop(&mut self) {
        if let Ok(mut map) = self.slots.lock()
            && let Some(count) = map.get_mut(&self.ip)
        {
            *count -= 1;
            if *count == 0 {
                map.remove(&self.ip);
            }
        }
    }
}

fn rotation_delay(rng: &mut impl Rng) -> Duration {
    Duration::from_secs(rng.gen_range(ROTATION_MIN_SECONDS..=ROTATION_MAX_SECONDS))
}
fn may_rotate(inbound: bool, active_peers: usize) -> bool {
    // Rotate our outbound links only, and retain another validated connection.
    !inbound && active_peers >= 2
}
fn reserve_rotation(last: &mut Option<std::time::Instant>, now: std::time::Instant) -> bool {
    if last.is_some_and(|time| now.saturating_duration_since(time) < Duration::from_secs(60)) {
        return false;
    }
    *last = Some(now);
    true
}
fn randomized_candidates(known: &[PeerAddr], rng: &mut impl Rng) -> Vec<PeerAddr> {
    let mut candidates = known.to_vec();
    candidates.shuffle(rng);
    candidates
}
fn bounded_cache(
    known: &BTreeSet<PeerAddr>,
    explicit: &BTreeSet<PeerAddr>,
    rng: &mut impl Rng,
) -> BTreeSet<PeerAddr> {
    debug_assert!(explicit.len() <= MAX_KNOWN);
    let mut result = explicit.clone();
    let candidates = known.difference(explicit).cloned().collect::<Vec<_>>();
    for address in randomized_candidates(&candidates, rng) {
        if result.len() >= MAX_KNOWN {
            break;
        }
        admit_discovered(&mut result, address);
    }
    result
}
fn admit_discovered(known: &mut BTreeSet<PeerAddr>, address: PeerAddr) {
    if known.len() >= MAX_KNOWN || known.contains(&address) {
        return;
    }
    if address.is_onion()
        && known.iter().filter(|entry| entry.is_onion()).count() >= MAX_KNOWN_ONION
    {
        return;
    }
    if let Some(group) = netgroup(&address)
        && known
            .iter()
            .filter(|entry| netgroup(entry) == Some(group))
            .count()
            >= MAX_PER_NETGROUP
    {
        return;
    }
    known.insert(address);
}
fn dial_candidates(
    known: &[PeerAddr],
    explicit: &BTreeSet<PeerAddr>,
    rng: &mut impl Rng,
) -> Vec<PeerAddr> {
    let mut result = randomized_candidates(&explicit.iter().cloned().collect::<Vec<_>>(), rng);
    let discovered = known
        .iter()
        .filter(|address| !explicit.contains(address))
        .cloned()
        .collect::<Vec<_>>();
    result.extend(randomized_candidates(&discovered, rng));
    result
}
/// Whether another outbound connection to `address` keeps the outbound set
/// diverse. Explicit operator peers are exempt.
fn outbound_diverse(active: &BTreeSet<PeerAddr>, address: &PeerAddr) -> bool {
    let Some(group) = netgroup(address) else {
        return true;
    };
    active
        .iter()
        .filter(|entry| netgroup(entry) == Some(group))
        .count()
        < MAX_OUTBOUND_PER_NETGROUP
}
#[derive(Clone, Copy)]
struct DialRetry {
    failures: u8,
    after: std::time::Instant,
}
fn next_retry(previous: Option<DialRetry>, now: std::time::Instant, stable: bool) -> DialRetry {
    let failures = if stable {
        0
    } else {
        previous.map_or(1, |state| state.failures.saturating_add(1).min(8))
    };
    let seconds = (3u64 << failures.saturating_sub(1)).min(300);
    DialRetry {
        failures,
        after: now + Duration::from_secs(seconds),
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Hello {
    version: u16,
    network: Network,
    genesis: Hash,
    id: Hash,
    listen: SocketAddr,
    height: u64,
    tip: Hash,
}
/// Peer protocol v1 messages keep their order and encoding. Variants after
/// `Pong` are extensions, sent only to peers that announced `FEATURE_ADDR_V2`.
#[derive(Serialize, Deserialize)]
enum Message {
    Hello(Hello),
    GetBlocks {
        locators: Vec<Hash>,
    },
    Blocks {
        blocks: Vec<Block>,
    },
    Block(Box<Block>),
    Transaction(ShieldedTx),
    GetPeers,
    Peers(Vec<SocketAddr>),
    Ping(u64),
    Pong(u64),
    GetAddrV2,
    AddrV2(Vec<WireAddr>),
    /// The sender's own onion address; sent only over Tor links.
    Announce(WireAddr),
}
impl Message {
    fn is_extension(&self) -> bool {
        matches!(self, Self::GetAddrV2 | Self::AddrV2(_) | Self::Announce(_))
    }
}

async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    network: Network,
    message: &Message,
) -> Result<()> {
    let bytes = codec::encode(message)?;
    ensure!(
        bytes.len() <= MAX_FRAME,
        "outgoing peer message exceeds limit"
    );
    let mut frame = Vec::with_capacity(8 + bytes.len());
    frame.extend_from_slice(&network.magic());
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(&bytes);
    // Tor streams buffer writes until a cell fills; without the flush a
    // small handshake frame would never leave this process.
    tokio::time::timeout(Duration::from_secs(10), async {
        writer.write_all(&frame).await?;
        writer.flush().await
    })
    .await??;
    Ok(())
}
async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R, network: Network) -> Result<Message> {
    let mut magic = [0; 4];
    reader.read_exact(&mut magic).await?;
    ensure!(magic == network.magic(), "peer network magic mismatch");
    let len = reader.read_u32_le().await? as usize;
    ensure!(len > 0 && len <= MAX_FRAME, "peer frame exceeds limit");
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).await?;
    codec::decode(&bytes, MAX_FRAME)
}
/// The writing half of a connection, which refuses to send an extension message
/// to a peer that has not announced support for it.
struct Outbox<W> {
    writer: W,
    network: Network,
    remote_features: u64,
}
impl<W: AsyncWriteExt + Unpin> Outbox<W> {
    async fn send(&mut self, message: &Message) -> Result<()> {
        ensure!(
            !message.is_extension() || self.remote_features & FEATURE_ADDR_V2 != 0,
            "extension message for a peer that did not announce it"
        );
        write_frame(&mut self.writer, self.network, message).await
    }
}
/// The handshake reveals a listening address only on clearnet links of a node
/// that listens on clearnet. Tor links advertise `0.0.0.0:0`, which no peer
/// records, and use a separate identity, so the two cannot be linked.
fn hello(node: &Node, link: Link) -> Result<Hello> {
    let d = node
        .data
        .lock()
        .map_err(|_| anyhow::anyhow!("state poisoned"))?;
    let unadvertised = link.via_tor() || node.net.options.listen.is_none();
    Ok(Hello {
        version: 1,
        network: node.network,
        genesis: node.network.genesis(),
        id: if link.via_tor() {
            node.net.tor_id
        } else {
            node.id
        },
        listen: if unadvertised {
            SocketAddr::from(([0, 0, 0, 0], 0))
        } else {
            node.p2p_listen
        },
        height: d.chain.state.height,
        tip: d.chain.state.best_hash,
    })
}
fn locators(node: &Node) -> Result<Vec<Hash>> {
    let d = node
        .data
        .lock()
        .map_err(|_| anyhow::anyhow!("state poisoned"))?;
    let mut out = vec![];
    let mut index = d.chain.active.len();
    let mut step = 1;
    while index > 0 && out.len() < 31 {
        out.push(d.chain.active[index - 1].hash()?);
        index = index.saturating_sub(step);
        if out.len() > 10 {
            step *= 2;
        }
    }
    out.push(node.network.genesis());
    Ok(out)
}
fn discovery_address_allowed(network: Network, own: SocketAddr, address: SocketAddr) -> bool {
    let ip = normalized_ip(address.ip());
    if address.port() == 0
        || (address.port() == own.port() && ip == normalized_ip(own.ip()))
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return false;
    }
    // A loopback-bound devnet lab cannot be turned into an arbitrary outbound-port
    // scanner by peers. A devnet node bound to a public interface joins the shared
    // devnet and follows the same public-address rules as testnet.
    if network == Network::Devnet && normalized_ip(own.ip()).is_loopback() {
        return ip.is_loopback();
    }
    match ip {
        std::net::IpAddr::V4(ip) => {
            !ip.is_private() && !ip.is_link_local() && !ip.is_loopback() && !ip.is_broadcast()
        }
        std::net::IpAddr::V6(ip) => {
            !ip.is_loopback() && !ip.is_unique_local() && !ip.is_unicast_link_local()
        }
    }
}
/// Whether a learned address may enter the discovery cache: reachable in this
/// node's network mode, publicly routable (or onion), and not this node itself.
fn acceptable_discovery(node: &Node, address: &PeerAddr) -> bool {
    if !node.net.policy.reachable(address) {
        return false;
    }
    match address {
        PeerAddr::Ip(socket) => discovery_address_allowed(node.network, node.p2p_listen, *socket),
        PeerAddr::Onion { port, .. } => {
            *port != 0 && node.net.onion_address().as_ref() != Some(address)
        }
        PeerAddr::Name { .. } => false,
    }
}
/// A peer bound to a wildcard address advertises 0.0.0.0/::; it is reachable, if
/// at all, at the address its connection was observed from.
fn advertised_address(listen: SocketAddr, observed: SocketAddr) -> SocketAddr {
    if listen.ip().is_unspecified() {
        SocketAddr::new(observed.ip(), listen.port())
    } else {
        listen
    }
}
fn stem_candidates(node: &Node) -> Vec<Candidate> {
    node.peers
        .lock()
        .map(|peers| {
            peers
                .values()
                .map(|peer| Candidate {
                    connection: peer.connection,
                    inbound: peer.inbound,
                    link: peer.link,
                })
                .collect()
        })
        .unwrap_or_default()
}
pub fn local_fluff_scope(node: &Node) -> RelayScope {
    relay::fluff_scope(node.net.options.mode, &stem_candidates(node))
}
fn in_mempool(node: &Node, id: &Hash) -> bool {
    node.data
        .lock()
        .is_ok_and(|data| data.chain.mempool.contains_key(id))
}
/// Broadcast policy for a transaction created by this node; see `relay`. It
/// waits for an acceptable stem peer however long that takes (in tor-only mode
/// only Tor links qualify), then broadcasts it once when the embargo ends unless
/// an echo already relayed it. Peers connecting later get it with the mempool.
/// It stays pending, also on disk, until it leaves the mempool: mined, expired,
/// invalidated by a conflicting spend, or evicted.
pub async fn broadcast_local(node: Node, id: Hash, tx: ShieldedTx) {
    let mode = node.net.options.mode;
    let mut waiting = false;
    let stem = loop {
        if !in_mempool(&node, &id) {
            node.forget_local(&id);
            return;
        }
        if let Some(stem) =
            relay::choose_stem(mode, &stem_candidates(&node), &mut rand08::rngs::OsRng)
        {
            break stem;
        }
        if !std::mem::replace(&mut waiting, true) {
            tracing::info!("local transaction waiting for an acceptable relay peer");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    node.relay.set_stem(&id, stem);
    tokio::time::sleep(relay::random_delay(relay::STEM_MEAN)).await;
    let _ = node.events.send(Event::Stem {
        tx: tx.clone(),
        to: stem,
    });
    tokio::time::sleep(node.relay.embargo()).await;
    if in_mempool(&node, &id) && node.relay.mark_forwarded(&id) {
        let scope = local_fluff_scope(&node);
        tracing::info!(
            tor_links_only = scope == RelayScope::TorLinks,
            "embargo ended; broadcasting local transaction"
        );
        let _ = node.events.send(Event::Transaction {
            tx: tx.clone(),
            from: None,
            scope,
        });
    }
    while in_mempool(&node, &id) {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    node.forget_local(&id);
}
/// Pending transactions a newly connected peer receives: the whole mempool
/// except this node's own transactions still in their stem phase, and (in
/// hybrid mode) own transactions on links their broadcast scope excludes.
fn mempool_for(node: &Node, link: Link) -> Vec<(Hash, ShieldedTx)> {
    let local_scope = local_fluff_scope(node);
    let Ok(d) = node.data.lock() else {
        return Vec::new();
    };
    d.chain
        .mempool
        .iter()
        .filter(|(id, _)| {
            !node.relay.in_stem_phase(id) && (!node.relay.is_local(id) || local_scope.allows(link))
        })
        .map(|(id, tx)| (*id, tx.clone()))
        .collect()
}

async fn connection(
    node: Node,
    stream: PeerStream,
    address: PeerAddr,
    inbound: bool,
    link: Link,
) -> Result<()> {
    let (mut reader, writer) = tokio::io::split(stream);
    let mut out = Outbox {
        writer,
        network: node.network,
        remote_features: 0,
    };
    out.send(&Message::Hello(hello(&node, link)?)).await?;
    let remote = match tokio::time::timeout(
        Duration::from_secs(10),
        read_frame(&mut reader, node.network),
    )
    .await??
    {
        Message::Hello(h) => h,
        _ => anyhow::bail!("expected peer handshake"),
    };
    ensure!(
        remote.version == 1
            && remote.network == node.network
            && remote.genesis == node.network.genesis(),
        "incompatible peer"
    );
    ensure!(
        remote.id != node.id && remote.id != node.net.tor_id,
        "self connection"
    );
    let connection_id = node.next_connection.fetch_add(1, Ordering::Relaxed);
    {
        let mut peers = node
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("peer state poisoned"))?;
        ensure!(!peers.contains_key(&remote.id), "duplicate peer");
        ensure!(peers.len() < node.limits.connections, "connection limit");
        peers.insert(
            remote.id,
            Peer {
                network: if link == Link::OnionInbound {
                    NetClass::Onion
                } else {
                    address.class()
                },
                address: address.clone(),
                inbound,
                height: remote.height,
                link,
                connection: connection_id,
            },
        );
    }
    struct Guard {
        node: Node,
        id: Hash,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if let Ok(mut p) = self.node.peers.lock() {
                p.remove(&self.id);
            }
        }
    }
    let _guard = Guard {
        node: node.clone(),
        id: remote.id,
    };
    // Only a direct connection shows where a peer really is; through Tor the
    // observed address is the proxy or Tor's loopback forwarder.
    if link == Link::Clearnet
        && let Some(observed) = address.ip()
    {
        let advertised = PeerAddr::from(advertised_address(remote.listen, observed));
        if acceptable_discovery(&node, &advertised) {
            admit_discovered(
                &mut *node
                    .known
                    .lock()
                    .map_err(|_| anyhow::anyhow!("peer state poisoned"))?,
                advertised,
            );
        }
    }
    tracing::info!(
        peer = %node.net.log_peer(&address),
        inbound,
        link = link.name(),
        "peer connected"
    );
    let mut events = node.events.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let rotate = tokio::time::sleep(rotation_delay(&mut rand08::rngs::OsRng));
    tokio::pin!(rotate);
    let mut sent = BTreeSet::new();
    let mut strikes = 0u32;
    // A dedicated reader owns partial frames. Cancelling read_exact in select!
    // after consuming a frame prefix would corrupt stream alignment.
    let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(2);
    let network = node.network;
    let reader_task = tokio::spawn(async move {
        loop {
            let result = match tokio::time::timeout(
                Duration::from_secs(30),
                read_frame(&mut reader, network),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => Err(e.into()),
            };
            let failed = result.is_err();
            if incoming_tx.send(result).await.is_err() || failed {
                break;
            }
        }
    });
    struct ReaderGuard(tokio::task::JoinHandle<()>);
    impl Drop for ReaderGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _reader_guard = ReaderGuard(reader_task);
    let mut sync_tip = None;
    let opened = Instant::now();
    let mut get_blocks_window = RequestWindow::new(opened);
    let mut ping_window = RequestWindow::new(opened);
    let mut peer_list_window = RequestWindow::new(opened);
    let mut relays: RelayQueue<ShieldedTx> = RelayQueue::default();
    out.send(&Message::GetBlocks {
        locators: locators(&node)?,
    })
    .await?;
    out.send(&Message::GetPeers).await?;
    out.send(&Message::Pong(FEATURE_MAGIC | OUR_FEATURES))
        .await?;
    for (id, tx) in mempool_for(&node, link) {
        let delay = if inbound {
            node.relay.inbound_delay(&id)
        } else {
            relay::random_delay(relay::OUTBOUND_MEAN)
        };
        relays.push(Instant::now() + delay, id, tx);
    }
    // Only the answer to our own request is admitted into the discovery cache.
    let mut awaiting_peers = true;
    let mut awaiting_addr_v2 = false;
    let mut features_known = false;
    let mut announcement_seen = false;
    enum Ready {
        Message(Option<Result<Message>>),
        Event(Result<Event, tokio::sync::broadcast::error::RecvError>),
        Tick,
        Rotate,
        Relay,
    }
    loop {
        let next_relay = relays.next_due();
        let relay_due = tokio::time::Instant::from_std(next_relay.unwrap_or_else(Instant::now));
        let ready = tokio::select! {
            message = incoming_rx.recv() => Ready::Message(message),
            event = events.recv() => Ready::Event(event),
            _ = tick.tick() => Ready::Tick,
            _ = &mut rotate, if !inbound => Ready::Rotate,
            _ = tokio::time::sleep_until(relay_due), if next_relay.is_some() => Ready::Relay,
        };
        match ready {
            Ready::Message(message) => {
                let message = message.context("peer closed")??;
                match message {
                    Message::Hello(_) => anyhow::bail!("repeated handshake"),
                    Message::GetBlocks { .. }
                        if !get_blocks_window.allow(
                            Instant::now(),
                            REQUEST_WINDOW,
                            MAX_GET_BLOCKS_PER_WINDOW,
                        ) =>
                    {
                        strikes += 1;
                    }
                    Message::GetBlocks { locators } => {
                        ensure!(
                            !locators.is_empty() && locators.len() <= 32,
                            "invalid block locator"
                        );
                        let blocks = {
                            let d = node
                                .data
                                .lock()
                                .map_err(|_| anyhow::anyhow!("state poisoned"))?;
                            let mut start = None;
                            for locator in locators {
                                if locator == node.network.genesis() {
                                    start = Some(0);
                                    break;
                                }
                                if let Some(i) = d.chain.active_position(locator)? {
                                    start = Some(i + 1);
                                    break;
                                }
                            }
                            let start = start.context("no common genesis")?;
                            // One canonical block per frame bounds memory even for maximal blocks.
                            d.chain
                                .active
                                .iter()
                                .skip(start)
                                .take(1)
                                .cloned()
                                .collect::<Vec<_>>()
                        };
                        out.send(&Message::Blocks { blocks }).await?;
                    }
                    Message::Blocks { blocks } => {
                        ensure!(blocks.len() <= 1, "oversized block response");
                        let nonempty = !blocks.is_empty();
                        for block in blocks {
                            let hash = block.hash()?;
                            match node.accept_block_outcome(block).await {
                                Ok(outcome) => {
                                    if low_work_side_block(&outcome) {
                                        strikes += LOW_WORK_STRIKES;
                                    }
                                    sync_tip = Some(hash);
                                }
                                Err(error) => strikes += block_penalty(&error),
                            }
                        }
                        if nonempty {
                            let mut locator = locators(&node)?;
                            if let Some(tip) = sync_tip {
                                locator.insert(0, tip);
                                locator.truncate(32);
                            }
                            out.send(&Message::GetBlocks { locators: locator }).await?;
                        } else {
                            sync_tip = None;
                        }
                    }
                    Message::Block(block) => match node.accept_block_outcome(*block).await {
                        Ok(outcome) if low_work_side_block(&outcome) => {
                            strikes += LOW_WORK_STRIKES;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            out.send(&Message::GetBlocks {
                                locators: locators(&node)?,
                            })
                            .await?;
                            strikes += block_penalty(&error);
                        }
                    },
                    Message::Transaction(tx) => {
                        let id = tx.id()?;
                        let known = {
                            let d = node
                                .data
                                .lock()
                                .map_err(|_| anyhow::anyhow!("state poisoned"))?;
                            d.chain.mempool.contains_key(&id)
                                || tx
                                    .bundle
                                    .actions
                                    .iter()
                                    .any(|a| d.chain.state.nullifiers.contains(&a.nullifier))
                        };
                        if known {
                            if in_mempool(&node, &id)
                                && node.relay.note_received(&id, connection_id)
                            {
                                let _ = node.events.send(Event::Transaction {
                                    tx,
                                    from: Some(connection_id),
                                    scope: local_fluff_scope(&node),
                                });
                            }
                        } else if let Err(error) = node.relay_tx(tx, connection_id).await {
                            strikes += transaction_penalty(&error);
                        }
                    }
                    Message::GetPeers | Message::GetAddrV2
                        if !peer_list_window.allow(
                            Instant::now(),
                            PEER_LIST_WINDOW,
                            MAX_PEER_LISTS_PER_WINDOW,
                        ) => {}
                    Message::GetPeers => {
                        let candidates = node
                            .known
                            .lock()
                            .map_err(|_| anyhow::anyhow!("peers poisoned"))?
                            .iter()
                            .filter(|address| {
                                !address.is_onion() && acceptable_discovery(&node, address)
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        let list = randomized_candidates(&candidates, &mut rand08::rngs::OsRng)
                            .into_iter()
                            .filter_map(|address| address.ip())
                            .take(MAX_ADVERTISED)
                            .collect();
                        out.send(&Message::Peers(list)).await?;
                    }
                    Message::Peers(peers) => {
                        ensure!(peers.len() <= MAX_ADVERTISED, "too many advertised peers");
                        if !std::mem::take(&mut awaiting_peers) {
                            continue;
                        }
                        let mut known = node
                            .known
                            .lock()
                            .map_err(|_| anyhow::anyhow!("peers poisoned"))?;
                        for p in peers {
                            let address = PeerAddr::from(p);
                            if acceptable_discovery(&node, &address) {
                                admit_discovered(&mut known, address);
                            }
                        }
                    }
                    Message::GetAddrV2 | Message::AddrV2(_) | Message::Announce(_)
                        if !features_known =>
                    {
                        // Extensions before a feature announcement break the protocol order.
                        strikes += 1;
                    }
                    Message::GetAddrV2 => {
                        let candidates = node
                            .known
                            .lock()
                            .map_err(|_| anyhow::anyhow!("peers poisoned"))?
                            .iter()
                            .filter(|address| acceptable_discovery(&node, address))
                            .cloned()
                            .collect::<Vec<_>>();
                        let list = randomized_candidates(&candidates, &mut rand08::rngs::OsRng)
                            .iter()
                            .filter_map(WireAddr::from_peer)
                            .take(MAX_ADVERTISED)
                            .collect();
                        out.send(&Message::AddrV2(list)).await?;
                    }
                    Message::AddrV2(addresses) => {
                        ensure!(
                            addresses.len() <= MAX_ADVERTISED,
                            "too many advertised peers"
                        );
                        if !std::mem::take(&mut awaiting_addr_v2) {
                            continue;
                        }
                        let mut known = node
                            .known
                            .lock()
                            .map_err(|_| anyhow::anyhow!("peers poisoned"))?;
                        for address in addresses.iter().filter_map(|wire| wire.to_peer().ok()) {
                            if acceptable_discovery(&node, &address) {
                                admit_discovered(&mut known, address);
                            }
                        }
                    }
                    Message::Announce(wire) => {
                        if std::mem::replace(&mut announcement_seen, true) {
                            continue;
                        }
                        if let Ok(address @ PeerAddr::Onion { .. }) = wire.to_peer()
                            && acceptable_discovery(&node, &address)
                        {
                            admit_discovered(
                                &mut *node
                                    .known
                                    .lock()
                                    .map_err(|_| anyhow::anyhow!("peers poisoned"))?,
                                address,
                            );
                        }
                    }
                    Message::Ping(_)
                        if !ping_window.allow(
                            Instant::now(),
                            REQUEST_WINDOW,
                            MAX_PINGS_PER_WINDOW,
                        ) =>
                    {
                        strikes += 1;
                    }
                    Message::Ping(n) => out.send(&Message::Pong(n)).await?,
                    Message::Pong(n) if n & !0xFFFF_FFFF == FEATURE_MAGIC && !features_known => {
                        features_known = true;
                        out.remote_features = n & OUR_FEATURES;
                        if out.remote_features & FEATURE_ADDR_V2 != 0 {
                            if node.net.options.mode.uses_tor() {
                                awaiting_addr_v2 = true;
                                out.send(&Message::GetAddrV2).await?;
                            }
                            if link.via_tor()
                                && let Some(onion) = node.net.onion_address()
                                && let Some(wire) = WireAddr::from_peer(&onion)
                            {
                                out.send(&Message::Announce(wire)).await?;
                            }
                        }
                    }
                    Message::Pong(_) => (),
                }
                ensure!(strikes < MISBEHAVIOR_LIMIT, "peer misbehavior threshold");
            }
            Ready::Event(event) => {
                match event {
                    Ok(Event::Block(b)) => {
                        let id = b.hash()?;
                        if sent.insert(id) {
                            out.send(&Message::Block(b)).await?;
                        }
                    }
                    Ok(Event::Transaction { tx, from, scope }) => {
                        if from != Some(connection_id) && scope.allows(link) {
                            let id = tx.id()?;
                            let delay = if inbound {
                                node.relay.inbound_delay(&id)
                            } else {
                                relay::random_delay(relay::OUTBOUND_MEAN)
                            };
                            relays.push(Instant::now() + delay, id, tx);
                        }
                    }
                    Ok(Event::Stem { tx, to }) => {
                        if to == connection_id {
                            let id = tx.id()?;
                            if sent.insert(id) {
                                out.send(&Message::Transaction(tx)).await?;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        out.send(&Message::GetBlocks {
                            locators: locators(&node)?,
                        })
                        .await?;
                    }
                    Err(_) => break,
                }
                if sent.len() > 512 {
                    sent.clear();
                }
            }
            Ready::Relay => {
                for (id, tx) in relays.pop_due(Instant::now()) {
                    if sent.insert(id) {
                        out.send(&Message::Transaction(tx)).await?;
                    }
                }
            }
            Ready::Tick => {
                out.send(&Message::GetBlocks {
                    locators: locators(&node)?,
                })
                .await?;
            }
            Ready::Rotate => {
                let peer_count = node
                    .peers
                    .lock()
                    .map_err(|_| anyhow::anyhow!("peer state poisoned"))?
                    .len();
                let permitted = may_rotate(inbound, peer_count)
                    && reserve_rotation(
                        &mut *node
                            .last_peer_rotation
                            .lock()
                            .map_err(|_| anyhow::anyhow!("rotation state poisoned"))?,
                        std::time::Instant::now(),
                    );
                if permitted {
                    tracing::info!(
                        peer = %node.net.log_peer(&address),
                        "rotating outbound peer connection"
                    );
                    break;
                }
                // A small/local network can retain its sole useful peer indefinitely.
                rotate
                    .as_mut()
                    .reset(tokio::time::Instant::now() + rotation_delay(&mut rand08::rngs::OsRng));
            }
        }
    }
    Ok(())
}
async fn accept_on(listener: &Option<TcpListener>) -> std::io::Result<(PeerStream, SocketAddr)> {
    match listener {
        Some(listener) => {
            let (stream, address) = listener.accept().await?;
            Ok((tcp_stream(stream), address))
        }
        None => std::future::pending().await,
    }
}
/// An operator asked to retry now, or Tor just became usable again.
async fn reconnect_requested(node: &Node) {
    match &node.net.tor {
        Some(tor) => tokio::select! {
            _ = node.reconnect.notified() => {}
            _ = tor.became_ready.notified() => {}
        },
        None => node.reconnect.notified().await,
    }
}
/// The built-in Tor client's onion service hands over streams in-process; they
/// carry no address, so a placeholder stands in for logging and status.
async fn accept_built_in_onion(
    inbound: &mut Option<tokio::sync::mpsc::Receiver<PeerStream>>,
) -> std::io::Result<(PeerStream, SocketAddr)> {
    match inbound.as_mut() {
        Some(receiver) => match receiver.recv().await {
            Some(stream) => Ok((stream, SocketAddr::from(([127, 0, 0, 1], 0)))),
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}
fn persist_cache(node: &Node, known: &[PeerAddr]) -> Result<()> {
    let file = node.net.policy.cache_file();
    let temporary = node.data_dir.join(format!("{file}.tmp"));
    std::fs::write(&temporary, serde_json::to_vec(known)?)?;
    std::fs::rename(temporary, node.data_dir.join(file))?;
    Ok(())
}
enum Accepted {
    Clearnet(std::io::Result<(PeerStream, SocketAddr)>),
    Onion(std::io::Result<(PeerStream, SocketAddr)>),
    Tick,
}
pub async fn run(node: Node, explicit: Vec<PeerAddr>) -> Result<()> {
    let clearnet = match node.net.options.listen {
        Some(listen) => Some(TcpListener::bind(listen).await?),
        None => None,
    };
    let onion = match node.net.onion_target() {
        Some(target) => Some(
            TcpListener::bind(target)
                .await
                .context("cannot bind the onion-service target listener")?,
        ),
        None => None,
    };
    let mut built_in_onion = node
        .net
        .tor
        .as_ref()
        .filter(|tor| tor.config.built_in() && tor.config.onion_service)
        .and_then(|tor| tor.take_onion_inbound());
    if let Some(tor) = &node.net.tor {
        tor.start();
    }
    tokio::spawn({
        let node = node.clone();
        async move { node.restore_local().await }
    });
    let own = PeerAddr::from(node.p2p_listen);
    let explicit = explicit
        .into_iter()
        .filter(|address| *address != own)
        .collect::<BTreeSet<_>>();
    ensure!(
        explicit.len() <= MAX_KNOWN,
        "too many explicit peer addresses"
    );
    {
        let mut known = node
            .known
            .lock()
            .map_err(|_| anyhow::anyhow!("peer state poisoned"))?;
        known.retain(|address| acceptable_discovery(&node, address));
        *known = bounded_cache(&known, &explicit, &mut rand08::rngs::OsRng);
    }
    let explicit = Arc::new(explicit);
    let outbound = Arc::new(Mutex::new(BTreeSet::<PeerAddr>::new()));
    let slots = Arc::new(Semaphore::new(node.limits.connections));
    let inbound_slots = Arc::new(Semaphore::new(node.limits.effective_inbound()));
    let inbound_by_ip = Arc::new(Mutex::new(BTreeMap::<IpAddr, usize>::new()));
    let retries = Arc::new(Mutex::new(BTreeMap::<PeerAddr, DialRetry>::new()));
    let bans = Arc::new(Mutex::new(BTreeMap::<IpAddr, Instant>::new()));
    let mut onion_arrivals = RequestWindow::new(Instant::now());
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    if let Some(listen) = node.net.options.listen {
        tracing::info!(listen = %listen, "P2P listening");
    }
    tracing::info!(
        mode = node.net.options.mode.name(),
        clearnet_inbound = clearnet.is_some(),
        onion_inbound = onion.is_some() || built_in_onion.is_some(),
        "P2P started"
    );
    loop {
        let accepted = tokio::select! {
            accepted = accept_on(&clearnet) => Accepted::Clearnet(accepted),
            accepted = accept_on(&onion) => Accepted::Onion(accepted),
            accepted = accept_built_in_onion(&mut built_in_onion) => Accepted::Onion(accepted),
            _ = tick.tick() => Accepted::Tick,
            _ = reconnect_requested(&node) => {
                if let Ok(mut retries) = retries.lock() {
                    retries.clear();
                }
                Accepted::Tick
            }
        };
        match accepted {
            Accepted::Clearnet(Err(error)) | Accepted::Onion(Err(error)) => {
                // Descriptor exhaustion and similar errors are transient; never stop the node.
                tracing::warn!(error = %error.kind(), "inbound accept failed");
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Accepted::Clearnet(Ok((stream, address))) => {
                let ip = normalized_ip(address.ip());
                let banned = bans
                    .lock()
                    .map_err(|_| anyhow::anyhow!("ban state"))?
                    .get(&ip)
                    .is_some_and(|until| *until > Instant::now());
                if banned {
                    continue;
                }
                let Some(ip_slot) = InboundIpSlot::acquire(&inbound_by_ip, ip) else {
                    continue;
                };
                let Ok(inbound_permit) = inbound_slots.clone().try_acquire_owned() else {
                    continue;
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let node = node.clone();
                let bans = bans.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _inbound_permit = inbound_permit;
                    let _ip_slot = ip_slot;
                    let peer = PeerAddr::from(address);
                    if let Err(error) =
                        connection(node.clone(), stream, peer.clone(), true, Link::Clearnet).await
                    {
                        let text = error.to_string();
                        tracing::debug!(peer = %node.net.log_peer(&peer), error = %text, "peer disconnected");
                        if (text.contains("threshold") || text.contains("exceeds limit"))
                            && let Ok(mut bans) = bans.lock()
                        {
                            record_ban(&mut bans, ip, Instant::now());
                        }
                    }
                });
            }
            Accepted::Onion(Ok((stream, address))) => {
                if !onion_arrivals.allow(
                    Instant::now(),
                    Duration::from_secs(60),
                    MAX_ONION_INBOUND_PER_MINUTE,
                ) {
                    continue;
                }
                let Ok(inbound_permit) = inbound_slots.clone().try_acquire_owned() else {
                    continue;
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let node = node.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _inbound_permit = inbound_permit;
                    let peer = PeerAddr::from(address);
                    if let Err(error) =
                        connection(node, stream, peer, true, Link::OnionInbound).await
                    {
                        tracing::debug!(error = %error, "onion peer disconnected");
                    }
                });
            }
            Accepted::Tick => {
                let known = node
                    .known
                    .lock()
                    .map_err(|_| anyhow::anyhow!("peer state poisoned"))?
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                retries
                    .lock()
                    .map_err(|_| anyhow::anyhow!("retry state poisoned"))?
                    .retain(|address, _| {
                        known.binary_search(address).is_ok() || explicit.contains(address)
                    });
                persist_cache(&node, &known)?;
                // Preserve the stable cache on disk, but never let sorted address
                // order permanently exclude other eligible discovery candidates.
                for address in dial_candidates(&known, &explicit, &mut rand08::rngs::OsRng) {
                    if retries
                        .lock()
                        .map_err(|_| anyhow::anyhow!("retry state poisoned"))?
                        .get(&address)
                        .is_some_and(|retry| retry.after > std::time::Instant::now())
                    {
                        continue;
                    }
                    // Tor not ready (or a route the mode forbids): nothing is dialed and
                    // the address keeps its standing.
                    if !node.net.connector.may_dial(&address) {
                        continue;
                    }
                    {
                        let active = outbound
                            .lock()
                            .map_err(|_| anyhow::anyhow!("outbound state"))?;
                        if active.len() >= node.limits.outbound_connections {
                            break;
                        }
                        if active.contains(&address)
                            || (!explicit.contains(&address)
                                && !outbound_diverse(&active, &address))
                        {
                            continue;
                        }
                    }
                    let Ok(permit) = slots.clone().try_acquire_owned() else {
                        break;
                    };
                    if !outbound
                        .lock()
                        .map_err(|_| anyhow::anyhow!("outbound state"))?
                        .insert(address.clone())
                    {
                        continue;
                    }
                    let node = node.clone();
                    let outbound = outbound.clone();
                    let retries = retries.clone();
                    let explicit = explicit.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let known = node.known.clone();
                        let started = Instant::now();
                        let mut orderly = false;
                        let mut misbehaved = false;
                        let mut local_failure = false;
                        match node.net.connector.connect(&address).await {
                            Ok((stream, link)) => {
                                match connection(node.clone(), stream, address.clone(), false, link)
                                    .await
                                {
                                    Ok(()) => orderly = true,
                                    Err(error) => {
                                        misbehaved = error.to_string().contains("threshold");
                                    }
                                }
                            }
                            Err(error) => {
                                local_failure = error.is_local();
                                tracing::debug!(
                                    peer = %node.net.log_peer(&address),
                                    error = %error,
                                    "outbound connection failed"
                                );
                            }
                        }
                        if !local_failure && let Ok(mut retries) = retries.lock() {
                            let next = next_retry(
                                retries.get(&address).copied(),
                                Instant::now(),
                                orderly
                                    || (!misbehaved
                                        && started.elapsed() >= Duration::from_secs(60)),
                            );
                            if next.failures >= FORGET_AFTER_FAILURES
                                && !explicit.contains(&address)
                            {
                                retries.remove(&address);
                                if let Ok(mut known) = known.lock() {
                                    known.remove(&address);
                                }
                            } else {
                                retries.insert(address.clone(), next);
                            }
                        }
                        if let Ok(mut o) = outbound.lock() {
                            o.remove(&address);
                        }
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::addr::tests::test_onion;
    use rand08::{SeedableRng, rngs::StdRng};

    fn ip(text: &str) -> PeerAddr {
        PeerAddr::Ip(text.parse().unwrap())
    }
    fn local(port: u16) -> PeerAddr {
        PeerAddr::Ip(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    #[test]
    fn full_stale_cache_preserves_and_prioritizes_explicit_peer() {
        let cached = (1..=512).map(local).collect::<BTreeSet<_>>();
        let manual = local(65000);
        let explicit = BTreeSet::from([manual.clone()]);
        let mut observed = BTreeSet::new();
        for seed in 0..32 {
            let mut rng = StdRng::seed_from_u64(seed);
            let retained = bounded_cache(&cached, &explicit, &mut rng);
            assert_eq!(retained.len(), MAX_KNOWN);
            assert!(retained.contains(&manual));
            assert_eq!(
                dial_candidates(
                    &retained.iter().cloned().collect::<Vec<_>>(),
                    &explicit,
                    &mut rng
                )[0],
                manual
            );
            observed.extend(retained);
        }
        assert!(
            observed
                .iter()
                .any(|address| address.port() > 128 && address.port() < 65000)
        );
        let mut discovered = BTreeSet::new();
        for address in cached {
            admit_discovered(&mut discovered, address);
        }
        assert_eq!(
            discovered.len(),
            MAX_KNOWN,
            "all admission paths retain the same memory bound"
        );
    }
    #[test]
    fn retry_delay_is_bounded_and_resets_after_stable_connection() {
        let now = std::time::Instant::now();
        let mut previous = None;
        for _ in 0..100 {
            let retry = next_retry(previous, now, false);
            assert!(retry.failures <= 8);
            assert!((3..=300).contains(&retry.after.duration_since(now).as_secs()));
            previous = Some(retry);
        }
        assert_eq!(
            previous.unwrap().after.duration_since(now),
            Duration::from_secs(300)
        );
        let reset = next_retry(previous, now, true);
        assert_eq!(reset.failures, 0);
        assert_eq!(reset.after.duration_since(now), Duration::from_secs(3));
    }
    #[test]
    fn mapped_ipv4_obeys_the_same_discovery_policy() {
        let own = "127.0.0.1:29410".parse().unwrap();
        for address in [
            "[::ffff:127.0.0.1]:29420",
            "[::ffff:10.0.0.1]:29420",
            "[::ffff:169.254.1.1]:29420",
            "[::ffff:224.0.0.1]:29420",
            "[::ffff:0.0.0.0]:29420",
        ] {
            assert!(!discovery_address_allowed(
                Network::Testnet,
                own,
                address.parse().unwrap()
            ));
        }
        assert!(discovery_address_allowed(
            Network::Devnet,
            own,
            "[::ffff:127.0.0.1]:29420".parse().unwrap()
        ));
        assert!(!discovery_address_allowed(
            Network::Devnet,
            own,
            "[::ffff:127.0.0.1]:29410".parse().unwrap()
        ));
        assert!(discovery_address_allowed(
            Network::Devnet,
            own,
            "[::1]:29420".parse().unwrap()
        ));
        assert!(discovery_address_allowed(
            Network::Testnet,
            own,
            "[::ffff:8.8.8.8]:29420".parse().unwrap()
        ));
    }
    #[test]
    fn publicly_bound_devnet_discovers_public_peers_only() {
        let lab: SocketAddr = "127.0.0.1:29410".parse().unwrap();
        let public: SocketAddr = "0.0.0.0:29410".parse().unwrap();
        let internet: SocketAddr = "8.8.8.8:29410".parse().unwrap();
        let loopback: SocketAddr = "127.0.0.1:29420".parse().unwrap();
        assert!(!discovery_address_allowed(Network::Devnet, lab, internet));
        assert!(discovery_address_allowed(Network::Devnet, lab, loopback));
        assert!(discovery_address_allowed(Network::Devnet, public, internet));
        for address in [
            "127.0.0.1:29420",
            "[::1]:29420",
            "192.168.1.5:29410",
            "10.0.0.1:29410",
            "169.254.1.1:29410",
            "0.0.0.0:29410",
            "8.8.8.8:0",
        ] {
            assert!(
                !discovery_address_allowed(Network::Devnet, public, address.parse().unwrap()),
                "{address}"
            );
        }
    }
    #[test]
    fn wildcard_listener_is_advertised_at_its_observed_address() {
        let observed: SocketAddr = "203.0.113.7:51234".parse().unwrap();
        assert_eq!(
            advertised_address("0.0.0.0:29410".parse().unwrap(), observed),
            "203.0.113.7:29410".parse().unwrap()
        );
        assert_eq!(
            advertised_address("[::]:29410".parse().unwrap(), observed),
            "203.0.113.7:29410".parse().unwrap()
        );
        let explicit: SocketAddr = "198.51.100.2:29410".parse().unwrap();
        assert_eq!(advertised_address(explicit, observed), explicit);
        // A Tor-link handshake advertises 0.0.0.0:0, which never becomes dialable.
        assert_eq!(
            advertised_address("0.0.0.0:0".parse().unwrap(), observed).port(),
            0
        );
        assert!(!discovery_address_allowed(
            Network::Mainnet,
            "0.0.0.0:29610".parse().unwrap(),
            advertised_address("0.0.0.0:0".parse().unwrap(), observed)
        ));
    }
    #[test]
    fn selection_is_not_fixed_to_sorted_prefix_and_preserves_candidates() {
        let known = (1..=32).map(local).collect::<Vec<_>>();
        let expected = known.iter().cloned().collect::<BTreeSet<_>>();
        let mut selected = BTreeSet::new();
        for seed in 0..32 {
            let shuffled = randomized_candidates(&known, &mut StdRng::seed_from_u64(seed));
            assert_eq!(shuffled.iter().cloned().collect::<BTreeSet<_>>(), expected);
            selected.extend(
                shuffled
                    .into_iter()
                    .take(crate::config::ResourceLimits::default().outbound_connections),
            );
        }
        assert_eq!(selected, expected);
        assert_eq!(known[0].port(), 1, "the persisted cache is not mutated");
    }
    #[test]
    fn rotation_is_bounded_and_preserves_the_sole_peer() {
        let mut rng = StdRng::seed_from_u64(9);
        for _ in 0..1000 {
            let seconds = rotation_delay(&mut rng).as_secs();
            assert!((ROTATION_MIN_SECONDS..=ROTATION_MAX_SECONDS).contains(&seconds));
        }
        assert!(!may_rotate(false, 0));
        assert!(!may_rotate(false, 1));
        assert!(may_rotate(false, 2));
        assert!(!may_rotate(
            true,
            crate::config::ResourceLimits::default().connections
        ));
        let now = std::time::Instant::now();
        let mut last = None;
        assert!(reserve_rotation(&mut last, now));
        assert!(
            !reserve_rotation(&mut last, now),
            "simultaneous deadlines must not rotate every peer"
        );
        assert!(!reserve_rotation(&mut last, now + Duration::from_secs(59)));
        assert!(reserve_rotation(&mut last, now + Duration::from_secs(60)));
    }
    #[test]
    fn one_netgroup_cannot_fill_the_discovery_cache() {
        let mut known = BTreeSet::new();
        for host in 0..=255u8 {
            admit_discovered(
                &mut known,
                PeerAddr::from(SocketAddr::from(([203, 0, 113, host], 29410))),
            );
            admit_discovered(
                &mut known,
                PeerAddr::from(SocketAddr::from(([203, 0, host, 1], 29410))),
            );
        }
        assert_eq!(known.len(), MAX_PER_NETGROUP);
        for second in 0..200u8 {
            admit_discovered(
                &mut known,
                PeerAddr::from(SocketAddr::from(([198, second, 0, 1], 29410))),
            );
        }
        assert_eq!(known.len(), MAX_KNOWN);
        let mapped = ip("[::ffff:203.0.200.9]:29410");
        let mut small = BTreeSet::new();
        for host in 1..=4u8 {
            admit_discovered(
                &mut small,
                PeerAddr::from(SocketAddr::from(([203, 0, 9, host], 29410))),
            );
        }
        admit_discovered(&mut small, mapped.clone());
        assert!(!small.contains(&mapped), "mapped IPv4 shares its /16 group");
        let mut v6 = BTreeSet::new();
        for n in 0..10u16 {
            let address = SocketAddr::new(
                std::net::Ipv6Addr::new(0x2001, 0xdb8, n, 0, 0, 0, 0, 1).into(),
                29410,
            );
            admit_discovered(&mut v6, PeerAddr::from(address));
        }
        assert_eq!(v6.len(), MAX_PER_NETGROUP);
    }
    #[test]
    fn onion_identities_cannot_fill_more_than_their_share_of_the_cache() {
        let mut known = BTreeSet::new();
        for seed in 0..=255u8 {
            admit_discovered(&mut known, PeerAddr::onion(test_onion(seed), 29610));
        }
        let onions = known.iter().filter(|address| address.is_onion()).count();
        assert!(onions <= MAX_KNOWN_ONION, "{onions}");
        for second in 0..200u8 {
            admit_discovered(
                &mut known,
                PeerAddr::from(SocketAddr::from(([198, second, 0, 1], 29410))),
            );
        }
        assert_eq!(known.len(), MAX_KNOWN, "clearnet peers keep the other half");
    }
    #[test]
    fn outbound_connections_spread_across_network_groups() {
        let mut active = BTreeSet::new();
        let first = ip("203.0.113.1:29610");
        assert!(outbound_diverse(&active, &first));
        active.insert(first);
        active.insert(ip("203.0.200.1:29610"));
        assert!(!outbound_diverse(&active, &ip("203.0.7.7:29610")));
        assert!(outbound_diverse(&active, &ip("198.51.100.1:29610")));
        assert!(
            outbound_diverse(&active, &local(1)),
            "loopback labs are exempt"
        );
    }

    #[test]
    fn request_windows_bound_and_reset() {
        let start = Instant::now();
        let mut window = RequestWindow::new(start);
        for _ in 0..MAX_GET_BLOCKS_PER_WINDOW {
            assert!(window.allow(start, REQUEST_WINDOW, MAX_GET_BLOCKS_PER_WINDOW));
        }
        assert!(!window.allow(start, REQUEST_WINDOW, MAX_GET_BLOCKS_PER_WINDOW));
        assert!(window.allow(
            start + REQUEST_WINDOW,
            REQUEST_WINDOW,
            MAX_GET_BLOCKS_PER_WINDOW
        ));
    }

    #[test]
    fn ban_table_and_inbound_slots_are_bounded() {
        let now = Instant::now();
        let mut bans = BTreeMap::new();
        for n in 0..(MAX_BANS as u32 + 50) {
            record_ban(&mut bans, IpAddr::from(n.to_be_bytes()), now);
        }
        assert_eq!(bans.len(), MAX_BANS);
        record_ban(
            &mut bans,
            "198.51.100.1".parse().unwrap(),
            now + Duration::from_secs(BAN_SECONDS + 1),
        );
        assert_eq!(bans.len(), 1, "expired bans are pruned before insertion");

        let slots = Arc::new(Mutex::new(BTreeMap::new()));
        let remote: IpAddr = "203.0.113.5".parse().unwrap();
        let held: Vec<_> = (0..MAX_INBOUND_PER_IP)
            .map(|_| InboundIpSlot::acquire(&slots, remote).unwrap())
            .collect();
        assert!(InboundIpSlot::acquire(&slots, remote).is_none());
        assert!(InboundIpSlot::acquire(&slots, "203.0.113.6".parse().unwrap()).is_some());
        let loopback: Vec<_> = (0..MAX_INBOUND_PER_IP * 4)
            .map(|_| InboundIpSlot::acquire(&slots, "127.0.0.1".parse().unwrap()))
            .collect();
        assert!(
            loopback.iter().all(Option::is_some),
            "local labs are exempt"
        );
        drop(held);
        drop(loopback);
        assert!(slots.lock().unwrap().is_empty());
        assert!(InboundIpSlot::acquire(&slots, remote).is_some());
    }

    #[test]
    fn only_invalid_data_is_punished() {
        use genoryn_core::chain::{PolicyRejection, UnknownParent};
        let policy = anyhow::Error::new(PolicyRejection("mempool full")).context("relay");
        assert_eq!(block_penalty(&policy), 0);
        assert_eq!(transaction_penalty(&policy), 0);
        assert_eq!(block_penalty(&anyhow::Error::new(UnknownParent)), 1);
        let invalid = anyhow::anyhow!("invalid proof of work");
        assert_eq!(block_penalty(&invalid), INVALID_BLOCK_STRIKES);
        assert_eq!(transaction_penalty(&invalid), INVALID_TRANSACTION_STRIKES);
    }

    #[test]
    fn only_far_behind_side_branches_earn_low_work_strikes() {
        use genoryn_core::chain::Acceptance;
        let outcome = |became_best, branch_work| BlockOutcome {
            acceptance: Acceptance {
                hash: [0; 32],
                became_best,
                reorg_depth: 0,
                branch_work,
            },
            tip_work: 1_000_000,
            next_difficulty: 100,
        };
        assert!(!low_work_side_block(&outcome(true, Some(10))));
        assert!(
            !low_work_side_block(&outcome(false, None)),
            "already stored"
        );
        assert!(
            !low_work_side_block(&outcome(false, Some(1_000_000 - 72_000))),
            "a competitive fork within the window"
        );
        assert!(low_work_side_block(&outcome(
            false,
            Some(1_000_000 - 72_001)
        )));
        assert!(low_work_side_block(&outcome(false, Some(5))));
    }

    #[test]
    fn extension_messages_follow_the_v1_variants() {
        let legacy = [
            (Message::GetPeers, 5u32),
            (Message::Ping(1), 7),
            (Message::Pong(1), 8),
            (Message::GetAddrV2, 9),
        ];
        for (message, index) in legacy {
            assert_eq!(codec::encode(&message).unwrap()[..4], index.to_le_bytes());
        }
        assert!(Message::GetAddrV2.is_extension());
        assert!(!Message::Pong(FEATURE_MAGIC | OUR_FEATURES).is_extension());
    }

    #[tokio::test]
    async fn outbox_refuses_extensions_before_a_feature_announcement() {
        let (writer, _reader) = tokio::io::duplex(4096);
        let mut out = Outbox {
            writer,
            network: Network::Devnet,
            remote_features: 0,
        };
        assert!(out.send(&Message::GetAddrV2).await.is_err());
        assert!(out.send(&Message::GetPeers).await.is_ok());
        out.remote_features = FEATURE_ADDR_V2;
        assert!(out.send(&Message::GetAddrV2).await.is_ok());
    }

    #[tokio::test]
    async fn malformed_and_oversized_frames_rejected() {
        let (mut write, mut read) = tokio::io::duplex(100);
        write.write_all(&Network::Devnet.magic()).await.unwrap();
        write.write_u32_le((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(read_frame(&mut read, Network::Devnet).await.is_err());
        let (mut write, mut read) = tokio::io::duplex(100);
        write.write_all(&Network::Testnet.magic()).await.unwrap();
        assert!(read_frame(&mut read, Network::Devnet).await.is_err());
    }

    #[tokio::test]
    async fn frames_reach_peers_through_buffering_streams() {
        // Like a Tor stream, BufWriter holds small writes until flushed.
        let (write, mut read) = tokio::io::duplex(1 << 16);
        let mut write = tokio::io::BufWriter::new(write);
        write_frame(&mut write, Network::Devnet, &Message::GetPeers)
            .await
            .unwrap();
        let received = tokio::time::timeout(
            Duration::from_secs(2),
            read_frame(&mut read, Network::Devnet),
        )
        .await
        .expect("frame stayed buffered")
        .unwrap();
        assert!(matches!(received, Message::GetPeers));
    }
}
