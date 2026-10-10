//! Light wallet transport against controlled remote nodes behind a local
//! stand-in for Tor's SOCKS port: fork choice by locally computed cumulative
//! work, rejection of sources serving invalid data, stale sources, and relaying
//! through another source. Every request must go through the proxy; nothing
//! contacts an external network.
mod support;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use genoryn::{client::RpcClient, light};
use genoryn_core::{
    block::Block,
    chain::{Chain, unix_time},
    params::Network,
    pow, privacy,
    state::ChainState,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use support::*;

#[derive(Clone)]
struct Remote {
    info: Value,
    /// Served by `/v1/blocks?from=N`: block N is `chain[N - 1]`.
    chain: Arc<Vec<Block>>,
    /// Replaces the `/v1/blocks` answer, for sources serving garbage.
    garbage: Option<Value>,
    relay_ok: bool,
    relayed: Arc<Mutex<usize>>,
}

async fn remote(remote: Remote) -> SocketAddr {
    async fn info(State(r): State<Remote>) -> Json<Value> {
        Json(r.info.clone())
    }
    async fn blocks(
        State(r): State<Remote>,
        Query(query): Query<HashMap<String, u64>>,
    ) -> Json<Value> {
        if let Some(garbage) = &r.garbage {
            return Json(garbage.clone());
        }
        let from = query.get("from").copied().unwrap_or(1).max(1) as usize;
        Json(json!(r.chain.get(from - 1).into_iter().collect::<Vec<_>>()))
    }
    async fn relay(State(r): State<Remote>) -> (axum::http::StatusCode, Json<Value>) {
        *r.relayed.lock().unwrap() += 1;
        if r.relay_ok {
            (
                axum::http::StatusCode::OK,
                Json(json!({"txid": "ab", "status": "relayed"})),
            )
        } else {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": "mempool full"})),
            )
        }
    }
    let router = Router::new()
        .route("/v1/info", get(info))
        .route("/v1/blocks", get(blocks))
        .route("/v1/relay", post(relay))
        .with_state(remote);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    address
}

fn info(network: Network, height: u64, best: &str) -> Value {
    json!({
        "chain_id": network.chain_id(),
        "genesis_hash": hex::encode(network.genesis()),
        "height": height,
        "best_hash": best,
    })
}

fn honest(network: Network) -> Remote {
    serving(network, Vec::new())
}
fn serving(network: Network, chain: Vec<Block>) -> Remote {
    let best = chain
        .last()
        .map_or_else(|| network.genesis(), |block| block.hash().unwrap());
    Remote {
        info: info(network, chain.len() as u64, &hex::encode(best)),
        chain: Arc::new(chain),
        garbage: None,
        relay_ok: true,
        relayed: Arc::default(),
    }
}

struct Setup {
    proxy: MockSocks,
    canary: Canary,
    labels: Vec<String>,
    clients: Vec<RpcClient>,
}
async fn setup(remotes: Vec<Remote>) -> Setup {
    let proxy = MockSocks::start().await;
    let canary = Canary::start().await;
    let mut labels = Vec::new();
    let mut clients = Vec::new();
    for (index, r) in remotes.into_iter().enumerate() {
        let local = remote(r).await;
        let host = format!("{}:29412", test_onion(10 + index as u8).hostname());
        proxy.route(&host, local);
        let url = format!("http://{host}");
        let credentials = format!("socks5h://genoryn:{index}@{}", proxy.address);
        clients.push(RpcClient::through_tor(&url, &credentials).unwrap());
        labels.push(url);
    }
    Setup {
        proxy,
        canary,
        labels,
        clients,
    }
}
async fn sync_from(
    network: Network,
    s: &Setup,
    local: Vec<Block>,
    state: ChainState,
) -> light::Verified {
    light::sync_verified(network, &s.clients, &s.labels, local, state)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_source_serving_invalid_data_is_rejected_and_never_followed() {
    let network = Network::Devnet;
    let liar = Remote {
        info: info(network, 2, &"11".repeat(32)),
        garbage: Some(json!([{"not": "a block"}])),
        ..honest(network)
    };
    let s = setup(vec![liar, honest(network)]).await;
    let verified = sync_from(network, &s, Vec::new(), ChainState::new(network)).await;
    assert_eq!(verified.used, None, "nothing valid to adopt");
    assert!(verified.blocks.is_empty());
    assert!(
        verified.reports[0].status.starts_with("rejected"),
        "{:?}",
        verified.reports[0]
    );
    assert_eq!(verified.reports[0].height, Some(2));
    assert_eq!(verified.reports[1].status, "agrees");
    assert!(!verified.is_stale());
    assert_eq!(s.canary.count(), 0);
    assert!(
        s.proxy
            .requests()
            .iter()
            .all(|r| matches!(&r.target, SocksTarget::Domain(host, _) if host.ends_with(".onion"))),
        "every request went to Tor by onion name"
    );
}

#[tokio::test]
async fn a_sync_no_source_confirmed_is_stale_never_a_verified_zero() {
    let network = Network::Devnet;
    // Answers its status, then fails every block download.
    let broken = Remote {
        info: info(network, 2, &"11".repeat(32)),
        garbage: Some(json!({"error": "unavailable"})),
        ..honest(network)
    };
    let s = setup(vec![broken]).await;
    let verified = sync_from(network, &s, Vec::new(), ChainState::new(network)).await;
    assert_eq!(verified.reachable, 1, "the source did answer its status");
    assert!(verified.is_stale(), "{:?}", verified.reports);
    assert_eq!(verified.used, None);
}

#[tokio::test]
async fn sources_on_another_network_or_a_forged_genesis_tip_are_reported() {
    let network = Network::Devnet;
    let other_network = Remote {
        info: info(
            Network::Testnet,
            0,
            &hex::encode(Network::Testnet.genesis()),
        ),
        ..honest(network)
    };
    let forged = Remote {
        info: info(network, 0, &"22".repeat(32)),
        ..honest(network)
    };
    let s = setup(vec![honest(network), other_network, forged]).await;
    let verified = sync_from(network, &s, Vec::new(), ChainState::new(network)).await;
    let statuses: Vec<_> = verified.reports.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(statuses[0], "agrees");
    assert_eq!(statuses[1], "rejected: another network or genesis");
    assert!(statuses[2].starts_with("rejected"), "{statuses:?}");
    assert_eq!(verified.used, None);
}

#[tokio::test]
async fn unreachable_sources_keep_the_last_verified_chain_and_mark_it_stale() {
    let network = Network::Devnet;
    let s = setup(vec![honest(network)]).await;
    // Route the only source nowhere: the proxy refuses the connection.
    s.proxy.routes.lock().unwrap().clear();
    let verified = sync_from(network, &s, Vec::new(), ChainState::new(network)).await;
    assert!(verified.is_stale(), "no source answered");
    assert_eq!(verified.used, None);
    assert!(verified.reports[0].status.starts_with("unreachable"));
}

#[tokio::test]
async fn transactions_leave_through_another_source_than_the_sync_source() {
    let network = Network::Devnet;
    let synced = honest(network);
    let other = honest(network);
    let (synced_count, other_count) = (synced.relayed.clone(), other.relayed.clone());
    let s = setup(vec![synced, other]).await;
    let tx = json!({"placeholder": true});
    let (via, answer) = light::relay(&s.clients, 0, &tx).await.unwrap();
    assert_eq!(via, 1);
    assert_eq!(answer["status"], "relayed");
    assert_eq!(
        (*synced_count.lock().unwrap(), *other_count.lock().unwrap()),
        (0, 1)
    );

    let refusing = Remote {
        relay_ok: false,
        ..honest(network)
    };
    let fallback = honest(network);
    let (refused, used) = (refusing.relayed.clone(), fallback.relayed.clone());
    let s = setup(vec![fallback, refusing]).await;
    let (via, _) = light::relay(&s.clients, 0, &tx).await.unwrap();
    assert_eq!(via, 0, "the sync source only as a last resort");
    assert_eq!((*refused.lock().unwrap(), *used.lock().unwrap()), (1, 1));
    assert_eq!(s.canary.count(), 0);
}

fn mine(mut block: Block) -> Block {
    loop {
        let hash = pow::randomx(block.header.pow_seed, &block.header.bytes().unwrap()).unwrap();
        if pow::meets_difficulty(hash, block.header.difficulty) {
            return block;
        }
        block.header.nonce += 1;
    }
}
/// A real devnet chain of `length` blocks whose timestamps are `spacing`
/// seconds apart, starting `start` seconds in the past. Fast blocks raise the
/// difficulty after the first window; slow ones keep it at the minimum.
fn devnet_chain(length: usize, start: u64, spacing: u64) -> Vec<Block> {
    let dir = tempfile::tempdir().unwrap();
    let mut chain = Chain::open(&dir.path().join("chain.sqlite"), Network::Devnet).unwrap();
    let first = unix_time() - start;
    let mut blocks = Vec::new();
    for index in 0..length as u64 {
        let mut block = chain
            .template(privacy::treasury_address(Network::Devnet))
            .unwrap();
        block.header.timestamp = first + index * spacing;
        let block = mine(block);
        assert!(chain.accept(block.clone()).unwrap().became_best);
        blocks.push(block);
    }
    blocks
}
fn work(blocks: &[Block]) -> u128 {
    blocks.iter().map(|b| b.header.difficulty as u128).sum()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "mines real devnet blocks with Halo 2 coinbase proofs; explicit full suite"]
async fn fork_choice_follows_cumulative_work_not_height() {
    let network = Network::Devnet;
    let fast = tokio::task::spawn_blocking(|| devnet_chain(13, 60, 1));
    let slow = tokio::task::spawn_blocking(|| devnet_chain(17, 20_000, 600));
    let (fast, slow) = (fast.await.unwrap(), slow.await.unwrap());
    assert_eq!(work(&fast), 16, "13 blocks: eleven at 1, then 2 and 3");
    assert_eq!(work(&slow[..15]), 15);

    // A fresh wallet adopts the only valid chain.
    let s = setup(vec![serving(network, fast.clone())]).await;
    let verified = sync_from(network, &s, Vec::new(), ChainState::new(network)).await;
    assert_eq!(verified.used, Some(0));
    assert_eq!(verified.state.work, 16);
    assert_eq!(verified.blocks.len(), 13);
    let (local, state) = (verified.blocks, verified.state);

    // Higher but with less work: never followed.
    let s = setup(vec![serving(network, slow[..15].to_vec())]).await;
    let verified = sync_from(network, &s, local.clone(), state.clone()).await;
    assert_eq!(verified.used, None, "{:?}", verified.reports);
    assert_eq!(verified.reports[0].status, "less work; not followed");
    assert_eq!(verified.state.best_hash, state.best_hash);

    // Equal work: the chain already followed is kept.
    let s = setup(vec![serving(network, slow[..16].to_vec())]).await;
    let verified = sync_from(network, &s, local.clone(), state.clone()).await;
    assert_eq!(verified.used, None);
    assert_eq!(verified.reports[0].status, "equal work; current chain kept");

    // More work: reorganize to it, from the genesis fork point.
    let s = setup(vec![
        serving(network, slow[..15].to_vec()),
        serving(network, slow.clone()),
    ])
    .await;
    let verified = sync_from(network, &s, local.clone(), state.clone()).await;
    assert_eq!(verified.used, Some(1));
    assert_eq!(verified.fork, 0);
    assert_eq!(verified.state.work, 17);
    assert_eq!(verified.blocks, slow);
    assert_eq!(verified.reports[0].status, "behind");
    let (local, state) = (verified.blocks, verified.state);

    // Extending the followed chain downloads only the new blocks.
    let s = setup(vec![serving(network, slow.clone())]).await;
    let verified = sync_from(network, &s, local[..16].to_vec(), {
        let mut short = ChainState::new(network);
        let mut cache = genoryn_core::state::ProofCache::default();
        for block in &local[..16] {
            short
                .apply_block_in_place(
                    block,
                    unix_time(),
                    &mut cache,
                    genoryn_core::state::Verification::Stored,
                )
                .unwrap();
        }
        short
    })
    .await;
    assert_eq!(verified.used, Some(0));
    assert_eq!(verified.fork, 16);
    assert_eq!(verified.state.best_hash, state.best_hash);

    // A source serving a tampered block is rejected; the valid chain stays.
    let mut tampered = fast.clone();
    tampered.push(tampered[12].clone());
    let s = setup(vec![serving(network, tampered)]).await;
    let verified = sync_from(network, &s, local.clone(), state.clone()).await;
    assert_eq!(verified.used, None);
    assert!(
        verified.reports[0].status.contains("rejected"),
        "{:?}",
        verified.reports
    );
    assert!(verified.is_stale(), "no source confirmed the chain");

    // Every source down: the last verified chain, flagged stale.
    let s = setup(vec![serving(network, slow.clone())]).await;
    s.proxy.routes.lock().unwrap().clear();
    let verified = sync_from(network, &s, local.clone(), state.clone()).await;
    assert!(verified.is_stale());
    assert_eq!(verified.state.best_hash, state.best_hash);
}
