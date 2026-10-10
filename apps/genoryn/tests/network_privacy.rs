//! Executable network-privacy checks against controlled local stand-ins for Tor.
//! Every assertion observes real sockets; nothing contacts an external network.
mod support;

use genoryn::net::{Link, NetworkMode, PeerAddr, transport::ConnectError};
use genoryn_core::{block::Block, codec, params::Network, privacy::ShieldedTx};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, time::Duration};
use support::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const KEY_MARKER: &str = "ED25519-V3:U0VDUkVUS0VZTUFSS0VSU0VDUkVUS0VZTUFSS0VSU0VDUkVUS0VZTUFSS0VSU0VDUkVUS0VZTUFSS0VSU0VDUkVUS0VZ";

fn tor_status(node: &genoryn::node::Node) -> genoryn::net::tor::TorStatus {
    node.net.tor.as_ref().unwrap().snapshot()
}
fn names() -> PeerAddr {
    PeerAddr::Name {
        host: "seed.example.org".into(),
        port: 29410,
    }
}

#[tokio::test]
async fn tor_only_mode_never_dials_directly_even_without_tor() {
    let canary = Canary::start().await;
    let closed = loopback(free_port());
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        None,
        NetworkMode::TorOnly,
        Some(tor_config(closed)),
    );
    let run = tokio::spawn(genoryn::p2p::run(
        node.clone(),
        vec![
            PeerAddr::Ip(canary.address),
            PeerAddr::onion(test_onion(1), 29410),
            names(),
        ],
    ));
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert_eq!(
        canary.count(),
        0,
        "no direct TCP connection in tor-only mode"
    );
    let counters = node.net.connector.counters.snapshot();
    assert_eq!(counters.direct_attempts, 0);
    assert_eq!(
        counters.tor_attempts, 0,
        "nothing dialed while Tor is unavailable"
    );
    assert!(!tor_status(&node).ready);
    assert!(matches!(
        node.net
            .connector
            .connect(&PeerAddr::Ip(canary.address))
            .await,
        Err(ConnectError::TorUnavailable)
    ));
    assert_eq!(canary.count(), 0);
    assert!(dir.path().join("peers-tor.json").exists());
    assert!(
        !dir.path().join("peers.json").exists(),
        "tor-only peers never enter the clearnet cache"
    );
    run.abort();
}

#[tokio::test]
async fn socks_failures_never_fall_back_to_clearnet() {
    let canary = Canary::start().await;
    let socks = MockSocks::start().await;
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        None,
        NetworkMode::TorOnly,
        Some(tor_config(socks.address)),
    );
    let run = tokio::spawn(genoryn::p2p::run(
        node.clone(),
        vec![PeerAddr::Ip(canary.address)],
    ));
    let target = SocksTarget::Ip(canary.address);
    assert!(
        wait_until(Duration::from_secs(20), || socks
            .requests()
            .iter()
            .any(|request| request.target == target))
        .await,
        "the peer was requested from the proxy"
    );
    socks
        .refusal
        .store(0x01, std::sync::atomic::Ordering::Relaxed);
    assert!(
        wait_until(Duration::from_secs(30), || node
            .net
            .connector
            .counters
            .snapshot()
            .tor_failures
            >= 2)
        .await
    );
    assert_eq!(canary.count(), 0, "refusals never cause a direct retry");
    assert_eq!(node.net.connector.counters.snapshot().direct_attempts, 0);
    run.abort();
}

#[tokio::test]
async fn onion_and_name_destinations_are_resolved_by_the_proxy() {
    let socks = MockSocks::start().await;
    let onion = test_onion(9);
    for mode in [NetworkMode::Hybrid, NetworkMode::TorOnly] {
        let dir = tempfile::tempdir().unwrap();
        let listen = (mode == NetworkMode::Hybrid).then(|| loopback(free_port()));
        let node = open_node(dir.path(), listen, mode, Some(tor_config(socks.address)));
        node.net.tor.as_ref().unwrap().start();
        assert!(wait_until(Duration::from_secs(10), || node.net.connector.tor_usable()).await);
        let before = socks.requests().len();
        let result = node
            .net
            .connector
            .connect(&PeerAddr::onion(onion, 29610))
            .await;
        assert!(matches!(result, Err(ConnectError::Socks(_))));
        let requests = socks.requests();
        assert_eq!(
            requests[before].target,
            SocksTarget::Domain(onion.hostname(), 29610),
            "the onion name went to the proxy verbatim"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        None,
        NetworkMode::TorOnly,
        Some(tor_config(socks.address)),
    );
    node.net.tor.as_ref().unwrap().start();
    assert!(wait_until(Duration::from_secs(10), || node.net.connector.tor_usable()).await);
    let _ = node.net.connector.connect(&names()).await;
    let ip: SocketAddr = "203.0.113.20:29410".parse().unwrap();
    let _ = node.net.connector.connect(&PeerAddr::Ip(ip)).await;
    let requests = socks.requests();
    let tail = &requests[requests.len() - 2..];
    assert_eq!(
        tail[0].target,
        SocksTarget::Domain("seed.example.org".into(), 29410)
    );
    assert_eq!(tail[1].target, SocksTarget::Ip(ip));
    assert!(
        requests
            .iter()
            .all(|r| r.username.as_deref() == Some("genoryn"))
    );
    let passwords: std::collections::BTreeSet<_> =
        requests.iter().filter_map(|r| r.password.clone()).collect();
    assert_eq!(
        passwords.len(),
        requests.len(),
        "each connection gets its own circuit"
    );
    assert_eq!(node.net.connector.counters.snapshot().direct_attempts, 0);
}

#[tokio::test]
async fn auxiliary_https_is_sent_to_tor_by_name_and_fails_closed() {
    let socks = MockSocks::start().await;
    let proxy = format!("socks5h://{}", socks.address);
    let seeds = genoryn::seeds::fetch_published(Network::Mainnet, Some(&proxy)).await;
    assert!(seeds.is_empty());
    assert!(
        socks.requests().iter().any(|request| request.target
            == SocksTarget::Domain("raw.githubusercontent.com".into(), 443)),
        "socks5h: the host name reached the proxy, so it was not resolved locally"
    );
    let closed = format!("socks5h://{}", loopback(free_port()));
    assert!(
        genoryn::seeds::fetch_published(Network::Mainnet, Some(&closed))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn tor_proxy_restart_breaks_and_restores_tor_routed_peers() {
    let dir_b = tempfile::tempdir().unwrap();
    let b_address = loopback(free_port());
    let node_b = open_node(dir_b.path(), Some(b_address), NetworkMode::Direct, None);
    let run_b = tokio::spawn(genoryn::p2p::run(node_b, vec![]));
    let socks = MockSocks::start().await;
    socks.route(&b_address.to_string(), b_address);
    let dir_a = tempfile::tempdir().unwrap();
    let node_a = open_node(
        dir_a.path(),
        None,
        NetworkMode::TorOnly,
        Some(tor_config(socks.address)),
    );
    let run_a = tokio::spawn(genoryn::p2p::run(
        node_a.clone(),
        vec![PeerAddr::Ip(b_address)],
    ));
    assert!(wait_until(Duration::from_secs(30), || peer_count(&node_a) == 1).await);
    assert!(
        node_a
            .peers
            .lock()
            .unwrap()
            .values()
            .all(|peer| peer.link == Link::Tor && !peer.inbound)
    );
    let address = socks.shutdown();
    assert!(
        wait_until(Duration::from_secs(30), || peer_count(&node_a) == 0
            && !node_a.net.connector.tor_usable())
        .await,
        "losing Tor drops Tor-routed peers and pauses dialing"
    );
    assert!(tor_status(&node_a).last_error.is_some());
    let socks = MockSocks::start_at(address).await;
    socks.route(&b_address.to_string(), b_address);
    assert!(
        wait_until(Duration::from_secs(60), || peer_count(&node_a) == 1).await,
        "the node reconnects once Tor is back"
    );
    assert_eq!(
        tor_status(&node_a).last_error,
        None,
        "a recovered Tor shows no stale error"
    );
    assert_eq!(node_a.net.connector.counters.snapshot().direct_attempts, 0);
    run_a.abort();
    run_b.abort();
}

fn control_options(methods: &'static str, password: Option<&'static str>) -> ControlOptions {
    ControlOptions {
        methods,
        password,
        service: test_onion(42),
        generated_key: KEY_MARKER,
    }
}

#[tokio::test]
async fn control_port_restart_re_registers_the_same_onion_service() {
    let tor_dir = tempfile::tempdir().unwrap();
    let control =
        MockControl::start(tor_dir.path(), control_options("COOKIE,SAFECOOKIE", None)).await;
    let socks = MockSocks::start().await;
    let mut tor = tor_config(socks.address);
    tor.control = Some(control.address);
    tor.onion_service = true;
    let target = tor.onion_target;
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::TorOnly, Some(tor));
    node.net.tor.as_ref().unwrap().start();
    let expected = PeerAddr::onion(test_onion(42), 29410);
    assert!(
        wait_until(Duration::from_secs(15), || node.net.onion_address()
            == Some(expected.clone()))
        .await
    );
    assert_eq!(tor_status(&node).auth_method.as_deref(), Some("safecookie"));
    assert!(
        control
            .commands()
            .contains(&format!("ADD_ONION NEW:ED25519-V3 Port=29410,{target}"))
    );
    let stored = std::fs::read_to_string(dir.path().join("onion_v3_private_key")).unwrap();
    assert_eq!(stored, KEY_MARKER);
    assert!(
        !control
            .commands()
            .iter()
            .any(|line| line.contains("Detach")),
        "the service ends with the control connection"
    );
    let address = control.shutdown();
    assert!(
        wait_until(Duration::from_secs(20), || tor_status(&node).onion.state
            == "starting")
        .await
    );
    assert!(node.net.onion_address().is_none());
    let control = MockControl::start_at(
        tor_dir.path(),
        address,
        control_options("COOKIE,SAFECOOKIE", None),
    )
    .await;
    assert!(
        wait_until(Duration::from_secs(30), || node.net.onion_address()
            == Some(expected.clone()))
        .await
    );
    assert_eq!(tor_status(&node).reconnections, 1);
    assert!(
        control
            .commands()
            .contains(&format!("ADD_ONION {KEY_MARKER} Port=29410,{target}")),
        "the persisted key restores the same onion address"
    );
}

#[tokio::test]
async fn no_connection_is_attempted_until_tor_finishes_bootstrapping() {
    let tor_dir = tempfile::tempdir().unwrap();
    let control = MockControl::start(tor_dir.path(), control_options("SAFECOOKIE", None)).await;
    control
        .bootstrap
        .store(40, std::sync::atomic::Ordering::Relaxed);
    let socks = MockSocks::start().await;
    let mut tor = tor_config(socks.address);
    tor.control = Some(control.address);
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::TorOnly, Some(tor));
    let target: SocketAddr = "127.0.0.1:9".parse().unwrap();
    let run = tokio::spawn(genoryn::p2p::run(node.clone(), vec![PeerAddr::Ip(target)]));
    assert!(
        wait_until(Duration::from_secs(10), || tor_status(&node)
            .bootstrap_progress
            == Some(40))
        .await
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        socks.requests().is_empty(),
        "no circuit requested mid-bootstrap"
    );
    assert_eq!(node.net.connector.counters.snapshot().tor_attempts, 0);
    control
        .bootstrap
        .store(100, std::sync::atomic::Ordering::Relaxed);
    assert!(
        wait_until(Duration::from_secs(30), || socks
            .requests()
            .iter()
            .any(|request| request.target == SocksTarget::Ip(target)))
        .await
    );
    run.abort();
}

#[tokio::test]
async fn hostile_control_port_never_receives_a_cookie_derived_hash() {
    let tor_dir = tempfile::tempdir().unwrap();
    let control = MockControl::start(tor_dir.path(), control_options("SAFECOOKIE", None)).await;
    control
        .forge_server_hash
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let mut tor = tor_config(loopback(free_port()));
    tor.control = Some(control.address);
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::TorOnly, Some(tor));
    node.net.tor.as_ref().unwrap().start();
    assert!(
        wait_until(Duration::from_secs(10), || tor_status(&node).control
            == "failed")
        .await
    );
    assert!(
        tor_status(&node)
            .control_error
            .unwrap()
            .contains("SAFECOOKIE server authentication")
    );
    assert!(
        !control
            .commands()
            .iter()
            .any(|line| line.starts_with("AUTHENTICATE")),
        "no client hash is sent to an impostor"
    );
}

#[tokio::test]
async fn hybrid_mode_routes_each_destination_by_policy() {
    let canary = Canary::start().await;
    let socks = MockSocks::start().await;
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        Some(loopback(free_port())),
        NetworkMode::Hybrid,
        Some(tor_config(socks.address)),
    );
    node.net.tor.as_ref().unwrap().start();
    assert!(wait_until(Duration::from_secs(10), || node.net.connector.tor_usable()).await);
    let (_stream, link) = node
        .net
        .connector
        .connect(&PeerAddr::Ip(canary.address))
        .await
        .unwrap();
    assert_eq!(link, Link::Clearnet);
    assert!(wait_until(Duration::from_secs(5), || canary.count() == 1).await);
    assert!(
        socks.requests().is_empty(),
        "clearnet peers bypass the proxy in hybrid mode"
    );
    let onion = test_onion(11);
    assert!(
        node.net
            .connector
            .connect(&PeerAddr::onion(onion, 29610))
            .await
            .is_err()
    );
    assert_eq!(
        socks.requests()[0].target,
        SocksTarget::Domain(onion.hostname(), 29610)
    );
    assert!(matches!(
        node.net.connector.connect(&names()).await,
        Err(ConnectError::Policy(_))
    ));
    let counters = node.net.connector.counters.snapshot();
    assert_eq!((counters.direct_attempts, counters.tor_attempts), (1, 1));
    assert_eq!(canary.count(), 1);
}

#[tokio::test]
async fn direct_mode_refuses_onion_peers_without_any_network_activity() {
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        Some(loopback(free_port())),
        NetworkMode::Direct,
        None,
    );
    assert!(matches!(
        node.net
            .connector
            .connect(&PeerAddr::onion(test_onion(3), 29610))
            .await,
        Err(ConnectError::Policy(_))
    ));
    assert_eq!(node.net.connector.counters.snapshot().direct_attempts, 0);
}

// ----- Peer protocol v1, as deployed before this change -----

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
/// The current protocol including extensions, as an upgraded peer speaks it.
#[derive(Serialize, Deserialize)]
enum MessageV2 {
    Hello(HelloV1),
    GetBlocks { locators: Vec<[u8; 32]> },
    Blocks { blocks: Vec<Block> },
    Block(Box<Block>),
    Transaction(ShieldedTx),
    GetPeers,
    Peers(Vec<SocketAddr>),
    Ping(u64),
    Pong(u64),
    GetAddrV2,
    AddrV2(Vec<genoryn::net::WireAddr>),
    Announce(genoryn::net::WireAddr),
}
const FEATURES: u64 = 0x474E_5246_0000_0001;

async fn send<T: Serialize>(stream: &mut TcpStream, message: &T) {
    let bytes = codec::encode(message).unwrap();
    stream.write_all(&Network::Devnet.magic()).await.unwrap();
    stream.write_u32_le(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
}
async fn receive_raw(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut magic = [0u8; 4];
    stream.read_exact(&mut magic).await.ok()?;
    let length = stream.read_u32_le().await.ok()? as usize;
    let mut bytes = vec![0u8; length];
    stream.read_exact(&mut bytes).await.ok()?;
    Some(bytes)
}
fn hello(listen: SocketAddr) -> HelloV1 {
    HelloV1 {
        version: 1,
        network: Network::Devnet,
        genesis: Network::Devnet.genesis(),
        id: rand_id(),
        listen,
        height: 0,
        tip: Network::Devnet.genesis(),
    }
}
fn rand_id() -> [u8; 32] {
    rand08::random()
}

/// Runs an unupgraded v1 peer for `seconds` and returns what it observed.
async fn legacy_peer(listener: TcpListener, seconds: u64) -> (Vec<&'static str>, bool) {
    let (mut stream, _) = listener.accept().await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut seen = Vec::new();
    let mut undecodable = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut greeted = false;
    loop {
        let Ok(Some(bytes)) = tokio::time::timeout_at(deadline, receive_raw(&mut stream)).await
        else {
            break;
        };
        let Ok(message) = codec::decode::<MessageV1>(&bytes, 2_100_000) else {
            undecodable = true;
            break;
        };
        match message {
            MessageV1::Hello(_) => {
                seen.push("Hello");
                if !greeted {
                    greeted = true;
                    send(&mut stream, &MessageV1::Hello(hello(address))).await;
                    send(&mut stream, &MessageV1::GetPeers).await;
                }
            }
            MessageV1::GetBlocks { .. } => {
                seen.push("GetBlocks");
                send(&mut stream, &MessageV1::Blocks { blocks: vec![] }).await;
            }
            MessageV1::GetPeers => {
                seen.push("GetPeers");
                send(&mut stream, &MessageV1::Peers(vec![])).await;
            }
            MessageV1::Peers(_) => seen.push("Peers"),
            MessageV1::Pong(_) => seen.push("Pong"),
            MessageV1::Ping(n) => send(&mut stream, &MessageV1::Pong(n)).await,
            MessageV1::Blocks { .. } => seen.push("Blocks"),
            MessageV1::Block(_) | MessageV1::Transaction(_) => seen.push("Relay"),
        }
    }
    (seen, undecodable)
}

#[tokio::test]
async fn unupgraded_v1_peers_never_receive_unknown_messages() {
    for mode in [NetworkMode::Direct, NetworkMode::Hybrid] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let legacy = listener.local_addr().unwrap();
        let peer = tokio::spawn(legacy_peer(listener, 8));
        let dir = tempfile::tempdir().unwrap();
        let tor = (mode == NetworkMode::Hybrid).then(|| tor_config(loopback(free_port())));
        let node = open_node(dir.path(), Some(loopback(free_port())), mode, tor);
        let run = tokio::spawn(genoryn::p2p::run(node.clone(), vec![PeerAddr::Ip(legacy)]));
        assert!(wait_until(Duration::from_secs(10), || peer_count(&node) == 1).await);
        let (seen, undecodable) = peer.await.unwrap();
        assert!(
            !undecodable,
            "{mode}: every frame decoded with the v1 schema"
        );
        for expected in ["Hello", "GetBlocks", "GetPeers", "Pong", "Peers"] {
            assert!(
                seen.contains(&expected),
                "{mode}: missing {expected} in {seen:?}"
            );
        }
        run.abort();
    }
}

#[tokio::test]
async fn upgraded_hybrid_peers_exchange_onion_addresses() {
    let closed = loopback(free_port());
    let onion = PeerAddr::onion(test_onion(77), 29410);
    let dir_b = tempfile::tempdir().unwrap();
    let b_address = loopback(free_port());
    let node_b = open_node(
        dir_b.path(),
        Some(b_address),
        NetworkMode::Hybrid,
        Some(tor_config(closed)),
    );
    node_b.known.lock().unwrap().insert(onion.clone());
    let run_b = tokio::spawn(genoryn::p2p::run(node_b, vec![]));
    let dir_a = tempfile::tempdir().unwrap();
    let node_a = open_node(
        dir_a.path(),
        Some(loopback(free_port())),
        NetworkMode::Hybrid,
        Some(tor_config(closed)),
    );
    let run_a = tokio::spawn(genoryn::p2p::run(
        node_a.clone(),
        vec![PeerAddr::Ip(b_address)],
    ));
    assert!(
        wait_until(Duration::from_secs(20), || node_a
            .known
            .lock()
            .unwrap()
            .contains(&onion))
        .await,
        "the onion address arrived through the address-exchange extension"
    );
    assert_eq!(
        node_a.net.connector.counters.snapshot().tor_attempts,
        0,
        "learned onion peers are not dialed while Tor is unavailable"
    );
    run_a.abort();
    run_b.abort();
}

#[tokio::test]
async fn hostile_peer_messages_cannot_steer_a_tor_only_node_onto_clearnet() {
    let canary = Canary::start().await;
    let hostile = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hostile_address = hostile.local_addr().unwrap();
    let socks = MockSocks::start().await;
    socks.route(&hostile_address.to_string(), hostile_address);
    let announced = PeerAddr::onion(test_onion(55), 29410);
    let announced_wire = genoryn::net::WireAddr::from_peer(&announced).unwrap();
    let canary_address = canary.address;
    let attacker = tokio::spawn(async move {
        let (mut stream, _) = hostile.accept().await.unwrap();
        let _ = receive_raw(&mut stream).await; // the node's Hello
        // The handshake points at the canary; over a Tor link it must be ignored.
        send(&mut stream, &MessageV2::Hello(hello(canary_address))).await;
        send(&mut stream, &MessageV2::Pong(FEATURES)).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while let Ok(Some(bytes)) =
            tokio::time::timeout_at(deadline, receive_raw(&mut stream)).await
        {
            match codec::decode::<MessageV2>(&bytes, 2_100_000) {
                Ok(MessageV2::GetPeers) => {
                    send(
                        &mut stream,
                        &MessageV2::Peers(vec![
                            canary_address,
                            "10.0.0.1:29410".parse().unwrap(),
                            "0.0.0.0:0".parse().unwrap(),
                        ]),
                    )
                    .await
                }
                Ok(MessageV2::GetAddrV2) => {
                    let SocketAddr::V4(canary) = canary_address else {
                        unreachable!()
                    };
                    send(
                        &mut stream,
                        &MessageV2::AddrV2(vec![
                            genoryn::net::WireAddr::V4(canary.ip().octets(), canary.port()),
                            genoryn::net::WireAddr::OnionV3([0; 32], 29410),
                            genoryn::net::WireAddr::V4([127, 0, 0, 1], 0),
                        ]),
                    )
                    .await;
                    send(&mut stream, &MessageV2::Announce(announced_wire.clone())).await;
                }
                Ok(MessageV2::GetBlocks { .. }) => {
                    send(&mut stream, &MessageV2::Blocks { blocks: vec![] }).await
                }
                _ => {}
            }
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(
        dir.path(),
        None,
        NetworkMode::TorOnly,
        Some(tor_config(socks.address)),
    );
    let run = tokio::spawn(genoryn::p2p::run(
        node.clone(),
        vec![PeerAddr::Ip(hostile_address)],
    ));
    assert!(
        wait_until(Duration::from_secs(30), || socks
            .requests()
            .iter()
            .any(|request| request.target == SocksTarget::Ip(canary_address)))
        .await,
        "the learned address was dialed, and only through Tor"
    );
    assert!(
        wait_until(Duration::from_secs(10), || node
            .known
            .lock()
            .unwrap()
            .contains(&announced))
        .await
    );
    assert_eq!(canary.count(), 0);
    assert_eq!(node.net.connector.counters.snapshot().direct_attempts, 0);
    let known = node.known.lock().unwrap().clone();
    assert!(known.iter().all(|address| address.port() != 0));
    assert!(!known.contains(&PeerAddr::Ip("10.0.0.1:29410".parse().unwrap())));
    assert!(known.iter().all(
        |address| !matches!(address, PeerAddr::Onion { key, .. } if key.public_key() == [0; 32])
    ));
    run.abort();
    attacker.abort();
}

#[test]
fn credentials_keys_and_peer_identities_never_reach_logs() {
    let capture = LogCapture::default();
    let _guard = capture.install();
    // The peer node runs on its own thread so only the tested node's logs are captured.
    let b_address = loopback(free_port());
    let dir_b = tempfile::tempdir().unwrap();
    let b_path = dir_b.path().to_path_buf();
    let (stop_b, stopped) = std::sync::mpsc::channel::<()>();
    let peer_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let node_b = open_node(&b_path, Some(b_address), NetworkMode::Direct, None);
            let run = tokio::spawn(genoryn::p2p::run(node_b, vec![]));
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            run.abort();
        });
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let password = "correct-horse-battery-staple-test";
    let (status, onion, socks_address) = runtime.block_on(async {
        let tor_dir = tempfile::tempdir().unwrap();
        let control = MockControl::start(
            tor_dir.path(),
            control_options("HASHEDPASSWORD,SAFECOOKIE", Some(password)),
        )
        .await;
        let socks = MockSocks::start().await;
        socks.route(&b_address.to_string(), b_address);
        let password_file = tor_dir.path().join("control.password");
        std::fs::write(&password_file, format!("{password}\n")).unwrap();
        let mut tor = tor_config(socks.address);
        tor.control = Some(control.address);
        tor.control_password_file = Some(password_file);
        tor.onion_service = true;
        let dir = tempfile::tempdir().unwrap();
        let node = open_node(dir.path(), None, NetworkMode::TorOnly, Some(tor));
        let run = tokio::spawn(genoryn::p2p::run(
            node.clone(),
            vec![PeerAddr::Ip(b_address)],
        ));
        assert!(
            wait_until(Duration::from_secs(30), || peer_count(&node) == 1
                && node.net.onion_address().is_some())
            .await
        );
        assert_eq!(tor_status(&node).auth_method.as_deref(), Some("password"));
        let status = node.network_status().unwrap().to_string();
        let onion = node.net.onion_address().unwrap().to_string();
        run.abort();
        (status, onion, socks.address)
    });
    stop_b.send(()).unwrap();
    peer_thread.join().unwrap();
    let logs = capture.text();
    assert!(
        logs.contains("peer connected"),
        "the scenario produced logs: {logs}"
    );
    assert!(logs.contains("ipv4#"), "peers appear only as salted labels");
    let onion_host = onion.split(':').next().unwrap();
    for secret in [
        password,
        KEY_MARKER,
        &KEY_MARKER["ED25519-V3:".len()..],
        &hex::encode([0x5au8; 32]),
        onion_host,
        &b_address.to_string(),
        &socks_address.to_string(),
    ] {
        assert!(!logs.contains(secret), "log leaked {secret}");
    }
    for secret in [password, KEY_MARKER] {
        assert!(!status.contains(secret), "network status leaked {secret}");
    }
    assert!(
        status.contains(onion_host),
        "the loopback admin status shows the onion address"
    );
}

#[tokio::test]
async fn rejected_control_passwords_are_not_echoed_in_status() {
    let tor_dir = tempfile::tempdir().unwrap();
    let control = MockControl::start(
        tor_dir.path(),
        control_options("HASHEDPASSWORD", Some("the-right-one")),
    )
    .await;
    let password_file = tor_dir.path().join("control.password");
    let wrong = "wrong-password-must-not-leak";
    std::fs::write(&password_file, wrong).unwrap();
    let mut tor = tor_config(loopback(free_port()));
    tor.control = Some(control.address);
    tor.control_password_file = Some(password_file);
    let dir = tempfile::tempdir().unwrap();
    let node = open_node(dir.path(), None, NetworkMode::TorOnly, Some(tor));
    node.net.tor.as_ref().unwrap().start();
    assert!(
        wait_until(Duration::from_secs(10), || tor_status(&node).control
            == "failed")
        .await
    );
    let status = serde_json::to_string(&tor_status(&node)).unwrap();
    assert!(status.contains("rejected the control password"));
    assert!(!status.contains(wrong));
}
