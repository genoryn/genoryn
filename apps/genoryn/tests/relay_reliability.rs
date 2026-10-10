//! Adversarial delivery checks for transactions created by a node: suppression
//! by a dropping stem peer, outages, late peers and restarts. Real loopback
//! nodes against scripted peers speaking the deployed v1 protocol; nothing
//! contacts an external network.
mod support;

use genoryn::{
    net::{NetworkMode, PeerAddr},
    node::Node,
    relay,
};
use genoryn_core::{
    block::Block,
    codec::{self, Hash},
    params::Network,
    privacy::{ActionWire, BundleWire, ShieldedTx, TxContext},
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use support::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

#[derive(Serialize, Deserialize, Clone)]
struct HelloV1 {
    version: u16,
    network: Network,
    genesis: [u8; 32],
    id: [u8; 32],
    listen: SocketAddr,
    height: u64,
    tip: [u8; 32],
}
#[derive(Serialize, Deserialize)]
enum MessageV1 {
    Hello(HelloV1),
    GetBlocks { locators: Vec<[u8; 32]> },
    Blocks { blocks: Vec<Block> },
    Block(Box<Block>),
    Transaction(ShieldedTx),
    GetPeers,
    Peers(Vec<SocketAddr>),
    Ping(u64),
    Pong(u64),
}

/// Well-formed but unprovable; a node only relays it because the test placed
/// it in the mempool, exactly where a validated transaction would be.
fn transaction(seed: u8) -> ShieldedTx {
    ShieldedTx {
        context: TxContext {
            protocol: 1,
            network: Network::Devnet,
            kind: 0,
            height: 1,
            expires: 100,
            nonce: [seed; 32],
        },
        bundle: BundleWire {
            actions: vec![ActionWire {
                nullifier: [seed; 32],
                rk: [1; 32],
                cmx: [2; 32],
                ephemeral_key: [3; 32],
                encrypted_note: vec![4; 16],
                outgoing_ciphertext: vec![5; 16],
                value_commitment: [6; 32],
                signature: vec![7; 64],
            }],
            flags: 3,
            value_balance: 10_000_000,
            anchor: [8; 32],
            proof: vec![9; 32],
            binding_signature: vec![10; 64],
        },
    }
}
fn pend(node: &Node, tx: &ShieldedTx) -> Hash {
    let id = tx.id().unwrap();
    node.data
        .lock()
        .unwrap()
        .chain
        .mempool
        .insert(id, tx.clone());
    id
}
fn originate(node: &Node, tx: &ShieldedTx) -> Hash {
    let id = pend(node, tx);
    assert!(node.relay.register_local(id));
    tokio::spawn(genoryn::p2p::broadcast_local(node.clone(), id, tx.clone()));
    id
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// Records transactions; never sends any.
    Honest,
    /// Whoever receives the first transaction drops it; the next peer in the
    /// ring echoes it back to the node as if the network had it.
    Colluding,
}
#[derive(Default)]
struct Ring {
    /// Index of the peer that received the first transaction, when, and what.
    stem: Mutex<Option<(usize, Instant, ShieldedTx)>>,
}
type Seen = Arc<Mutex<Vec<(Instant, Hash)>>>;

fn saw(seen: &Seen, id: &Hash) -> bool {
    seen.lock().unwrap().iter().any(|(_, s)| s == id)
}

async fn send(writer: &mut (impl AsyncWriteExt + Unpin), message: &MessageV1) -> bool {
    let bytes = codec::encode(message).unwrap();
    let mut frame = Network::Devnet.magic().to_vec();
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(&bytes);
    writer.write_all(&frame).await.is_ok()
}

/// A v1 peer accepting one connection from the node under test.
async fn scripted_peer(
    listener: TcpListener,
    index: usize,
    role: Role,
    ring: Arc<Ring>,
    seen: Seen,
) {
    let listen = listener.local_addr().unwrap();
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let (mut reader, mut writer) = stream.into_split();
    let (messages, mut incoming) = mpsc::channel(64);
    tokio::spawn(async move {
        loop {
            let mut head = [0u8; 8];
            if reader.read_exact(&mut head).await.is_err() {
                break;
            }
            let length = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
            let mut bytes = vec![0u8; length];
            if reader.read_exact(&mut bytes).await.is_err() {
                break;
            }
            let Ok(message) = codec::decode::<MessageV1>(&bytes, 2_100_000) else {
                continue;
            };
            if messages.send(message).await.is_err() {
                break;
            }
        }
    });
    let mut echoed = false;
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            message = incoming.recv() => {
                let Some(message) = message else { return };
                let reply = match message {
                    MessageV1::Hello(_) => Some(MessageV1::Hello(HelloV1 {
                        version: 1,
                        network: Network::Devnet,
                        genesis: Network::Devnet.genesis(),
                        id: rand08::random(),
                        listen,
                        height: 0,
                        tip: Network::Devnet.genesis(),
                    })),
                    MessageV1::GetBlocks { .. } => Some(MessageV1::Blocks { blocks: vec![] }),
                    MessageV1::GetPeers => Some(MessageV1::Peers(vec![])),
                    MessageV1::Ping(n) => Some(MessageV1::Pong(n)),
                    MessageV1::Transaction(tx) => {
                        seen.lock().unwrap().push((Instant::now(), tx.id().unwrap()));
                        if role == Role::Colluding {
                            let mut stem = ring.stem.lock().unwrap();
                            if stem.is_none() {
                                *stem = Some((index, Instant::now(), tx));
                            }
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(reply) = reply && !send(&mut writer, &reply).await {
                    return;
                }
            }
            _ = poll.tick(), if role == Role::Colluding && !echoed => {
                let echo = ring.stem.lock().unwrap().as_ref()
                    .filter(|(stem, _, _)| (stem + 1) % 3 == index)
                    .map(|(_, _, tx)| tx.clone());
                if let Some(tx) = echo {
                    echoed = true;
                    if !send(&mut writer, &MessageV1::Transaction(tx)).await {
                        return;
                    }
                }
            }
        }
    }
}

async fn peers(count: usize, role: Role, ring: &Arc<Ring>) -> (Vec<PeerAddr>, Vec<Seen>) {
    let mut addresses = Vec::new();
    let mut seen = Vec::new();
    for index in 0..count {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        addresses.push(PeerAddr::Ip(listener.local_addr().unwrap()));
        let log: Seen = Arc::default();
        seen.push(log.clone());
        tokio::spawn(scripted_peer(listener, index, role, ring.clone(), log));
    }
    (addresses, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropping_stem_and_an_echoing_accomplice_cannot_suppress_a_local_transaction() {
    let ring = Arc::new(Ring::default());
    let (addresses, seen) = peers(3, Role::Colluding, &ring).await;
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::Direct, None);
    let run = tokio::spawn(genoryn::p2p::run(node.clone(), addresses));
    assert!(wait_until(Duration::from_secs(15), || peer_count(&node) == 3).await);
    let id = originate(&node, &transaction(1));
    assert!(
        wait_until(Duration::from_secs(20), || ring
            .stem
            .lock()
            .unwrap()
            .is_some())
        .await
    );
    let (stem, stem_time) = {
        let stem = ring.stem.lock().unwrap();
        let (index, time, _) = stem.as_ref().unwrap();
        (*index, *time)
    };
    let honest = &seen[(stem + 2) % 3];
    // The echo is relayed onward like any transaction: before the shortest
    // embargo could have ended, so this is not the embargo broadcast.
    let shortest_embargo = Duration::from_secs(*relay::EMBARGO_SECONDS.start());
    let deadline = (shortest_embargo - Duration::from_secs(1)).saturating_sub(stem_time.elapsed());
    assert!(
        wait_until(deadline, || saw(honest, &id)).await,
        "suppressed: the honest peer never received the transaction"
    );
    assert_eq!(
        seen[stem].lock().unwrap().len(),
        1,
        "the stem is not sent it twice"
    );
    assert_eq!(node.net.connector.counters.snapshot().tor_attempts, 0);
    run.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silently_dropping_stem_is_bypassed_when_the_embargo_ends() {
    let ring = Arc::new(Ring::default());
    let (addresses, seen) = peers(3, Role::Honest, &ring).await;
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::Direct, None);
    let run = tokio::spawn(genoryn::p2p::run(node.clone(), addresses));
    assert!(wait_until(Duration::from_secs(15), || peer_count(&node) == 3).await);
    let id = originate(&node, &transaction(2));
    // Stem delay (<= 12 s) + embargo (<= 45 s) + relay delay (<= 12 s).
    assert!(
        wait_until(Duration::from_secs(75), || seen.iter().all(|s| saw(s, &id))).await,
        "every peer receives it even if the stem never relays"
    );
    let first = seen
        .iter()
        .filter_map(|s| s.lock().unwrap().first().map(|(time, _)| *time))
        .min()
        .unwrap();
    let last = seen
        .iter()
        .filter_map(|s| s.lock().unwrap().first().map(|(time, _)| *time))
        .max()
        .unwrap();
    assert!(
        last - first
            >= Duration::from_secs(*relay::EMBARGO_SECONDS.start()) - Duration::from_secs(1),
        "only the stem may receive it before the embargo"
    );
    run.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_peers_get_the_mempool_but_never_a_transaction_in_its_stem_phase() {
    let ring = Arc::new(Ring::default());
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::Direct, None);
    let relayed = pend(&node, &transaction(3));
    let stem_phase = pend(&node, &transaction(4));
    assert!(node.relay.register_local(stem_phase));
    let broadcast = pend(&node, &transaction(5));
    assert!(node.relay.register_local(broadcast) && node.relay.mark_forwarded(&broadcast));
    let (addresses, seen) = peers(1, Role::Honest, &ring).await;
    let run = tokio::spawn(genoryn::p2p::run(node.clone(), addresses));
    assert!(
        wait_until(Duration::from_secs(30), || saw(&seen[0], &relayed)
            && saw(&seen[0], &broadcast))
        .await,
        "a peer connecting later still receives pending transactions"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !saw(&seen[0], &stem_phase),
        "a transaction in its stem phase reaches no one but its stem"
    );
    run.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tor_only_delivery_waits_out_a_tor_outage_without_touching_clearnet() {
    let canary = Canary::start().await;
    let ring = Arc::new(Ring::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hidden = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
    tokio::spawn(scripted_peer(listener, 0, Role::Honest, ring, seen.clone()));
    let onion = PeerAddr::onion(test_onion(7), 29410);
    let socks = loopback(free_port());
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        None,
        NetworkMode::TorOnly,
        Some(tor_config(socks)),
    );
    let run = tokio::spawn(genoryn::p2p::run(
        node.clone(),
        vec![onion.clone(), PeerAddr::Ip(canary.address)],
    ));
    let id = originate(&node, &transaction(6));
    // Longer than the previous policy's 30 s search for a relay peer.
    tokio::time::sleep(Duration::from_secs(35)).await;
    assert_eq!(node.relay.pending(), 1, "still pending, not abandoned");
    assert!(node.relay.in_stem_phase(&id));
    assert!(!saw(&seen, &id));
    let proxy = MockSocks::start_at(socks).await;
    proxy.route(&onion.to_string(), hidden);
    assert!(
        wait_until(Duration::from_secs(60), || saw(&seen, &id)).await,
        "delivered through Tor once it is available again"
    );
    assert_eq!(
        canary.count(),
        0,
        "never a direct connection, not even to IP peers"
    );
    assert_eq!(node.net.connector.counters.snapshot().direct_attempts, 0);
    assert!(
        proxy
            .requests()
            .iter()
            .any(|request| matches!(&request.target, SocksTarget::Domain(host, _) if host.ends_with(".onion"))),
        "the onion peer is resolved by the proxy, not locally"
    );
    run.abort();
}

#[tokio::test]
async fn saved_local_transactions_that_no_longer_validate_are_dropped_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("local-transactions.json");
    let unprovable = hex::encode(codec::encode(&transaction(7)).unwrap());
    std::fs::write(
        &saved,
        serde_json::to_vec(&vec![unprovable, "zz".to_owned()]).unwrap(),
    )
    .unwrap();
    let node = open_node(dir.path(), None, NetworkMode::Direct, None);
    node.restore_local().await;
    assert_eq!(node.relay.pending(), 0);
    assert!(node.data.lock().unwrap().chain.mempool.is_empty());
    assert!(!saved.exists(), "nothing pending, nothing kept on disk");

    std::fs::write(&saved, b"{ not a list").unwrap();
    node.restore_local().await;
    assert!(!saved.exists());
}
