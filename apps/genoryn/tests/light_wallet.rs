//! Light wallet transport against controlled remote nodes behind a local
//! stand-in for Tor's SOCKS port: source selection, rejection of a source that
//! serves an invalid chain, cross-checks, and relaying through another source.
//! Every request must go through the proxy; nothing contacts an external network.
mod support;

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use genoryn::{client::RpcClient, light};
use genoryn_core::params::Network;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use support::*;

#[derive(Clone)]
struct Remote {
    info: Value,
    /// Response body for `/v1/blocks`.
    blocks: Value,
    relay_ok: bool,
    relayed: Arc<Mutex<usize>>,
}

async fn remote(remote: Remote) -> SocketAddr {
    async fn info(State(r): State<Remote>) -> Json<Value> {
        Json(r.info.clone())
    }
    async fn blocks(State(r): State<Remote>) -> Json<Value> {
        Json(r.blocks.clone())
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
    Remote {
        info: info(network, 0, &hex::encode(network.genesis())),
        blocks: json!([]),
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

#[tokio::test]
async fn a_source_serving_an_invalid_chain_is_rejected_and_another_is_used() {
    let network = Network::Devnet;
    let liar = Remote {
        info: info(network, 2, &"11".repeat(32)),
        blocks: json!([{"not": "a block"}]),
        ..honest(network)
    };
    let s = setup(vec![liar, honest(network)]).await;
    let mut scanned_lengths = Vec::new();
    let (scans, used, reports) = light::sync(network, &s.clients, &s.labels, |blocks| {
        scanned_lengths.push(blocks.len());
        Ok(blocks.len())
    })
    .await
    .unwrap();
    assert_eq!(used, 1, "the honest source");
    assert_eq!(scans, 0);
    assert!(
        reports[0].status.starts_with("rejected"),
        "{:?}",
        reports[0]
    );
    assert_eq!(reports[0].height, Some(2));
    assert_eq!(reports[1].status, "used");
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
async fn a_chain_that_fails_validation_is_never_accepted() {
    let network = Network::Devnet;
    let s = setup(vec![honest(network), honest(network)]).await;
    let result = light::sync(network, &s.clients, &s.labels, |_| -> anyhow::Result<()> {
        anyhow::bail!("proof invalid")
    })
    .await;
    assert!(
        result.is_err(),
        "no source passed validation, so no chain at all"
    );
}

#[tokio::test]
async fn sources_on_another_network_or_branch_are_reported() {
    let network = Network::Devnet;
    let other_network = Remote {
        info: info(
            Network::Testnet,
            0,
            &hex::encode(Network::Testnet.genesis()),
        ),
        ..honest(network)
    };
    let other_branch = Remote {
        info: info(network, 0, &"22".repeat(32)),
        ..honest(network)
    };
    let s = setup(vec![honest(network), other_network, other_branch]).await;
    let (_, used, reports) = light::sync(network, &s.clients, &s.labels, |blocks| Ok(blocks.len()))
        .await
        .unwrap();
    assert_eq!(reports[used].status, "used");
    let statuses: Vec<_> = reports.iter().map(|r| r.status.as_str()).collect();
    assert!(
        statuses.contains(&"rejected: another network or genesis"),
        "{statuses:?}"
    );
    assert!(statuses.contains(&"different branch"), "{statuses:?}");
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
