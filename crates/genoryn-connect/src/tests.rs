use super::*;
use ed25519_dalek::{Signer, SigningKey};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn config(endpoints: Vec<String>) -> ConnectConfig {
    ConnectConfig::devnet("test-chain", "11".repeat(32), endpoints)
}
fn info(height: u64) -> NodeInfo {
    NodeInfo {
        network: "devnet".into(),
        chain_id: "test-chain".into(),
        genesis_hash: "11".repeat(32),
        version: 1,
        height,
        best_hash: "22".repeat(32),
        difficulty: "1".into(),
        peer_count: 0,
        inbound_peers: None,
        outbound_peers: None,
        database_bytes: None,
        estimated_hashrate_hs: None,
        hashrate_estimate_basis: None,
        mempool_count: 0,
        public_rpc: false,
    }
}
#[test]
fn onion_endpoints_are_refused_before_any_system_dns_lookup() {
    for endpoint in [
        "https://duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion",
        "https://Anything.ONION.",
        "http://x.onion",
    ] {
        let error = validate_endpoint(endpoint, "devnet")
            .unwrap_err()
            .to_string();
        assert!(error.contains(".onion"), "{endpoint}: {error}");
    }
    assert!(validate_endpoint("https://onion.example.org", "devnet").is_ok());
    assert!(Connect::new(config(vec!["https://x.onion".into()])).is_err());
}
#[test]
fn optional_diagnostics_preserve_older_rpc_compatibility() {
    let mut value = serde_json::to_value(info(7)).unwrap();
    for field in [
        "inbound_peers",
        "outbound_peers",
        "database_bytes",
        "estimated_hashrate_hs",
        "hashrate_estimate_basis",
    ] {
        value.as_object_mut().unwrap().remove(field);
    }
    let legacy: NodeInfo = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(legacy.height, 7);
    assert!(legacy.database_bytes.is_none());
    value["inbound_peers"] = serde_json::json!(2);
    value["outbound_peers"] = serde_json::json!(3);
    value["database_bytes"] = serde_json::json!(123456);
    value["estimated_hashrate_hs"] = serde_json::json!(1.5);
    let extended: NodeInfo = serde_json::from_value(value).unwrap();
    assert_eq!(extended.inbound_peers, Some(2));
    assert_eq!(extended.outbound_peers, Some(3));
    assert_eq!(extended.database_bytes, Some(123456));
    assert_eq!(extended.estimated_hashrate_hs, Some(1.5));
}
async fn server(node: NodeInfo, supply_status: u16) -> (String, tokio::task::JoinHandle<()>) {
    server_delay(node, supply_status, 0).await
}
async fn server_delay(
    node: NodeInfo,
    supply_status: u16,
    delay_ms: u64,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let node = node.clone();
            tokio::spawn(async move {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                let (status, body) = if request.starts_with("GET /v1/info ") {
                    (200, serde_json::to_string(&node).unwrap())
                } else {
                    (supply_status, "{\"issued_supply\":\"100\"}".into())
                };
                let reply = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (address, handle)
}
#[test]
fn endpoint_policy_fails_closed() {
    for endpoint in [
        "http://example.com",
        "http://localhost:29411",
        "https://u:p@example.com",
        "https://example.com/base",
        "https://example.com?q=x",
        "https://example.com/#x",
        "file:///tmp/x",
    ] {
        assert!(validate_endpoint(endpoint, "devnet").is_err(), "{endpoint}");
    }
    assert!(validate_endpoint("http://127.0.0.1:29411", "devnet").is_ok());
    assert!(validate_endpoint("http://[::1]:29411", "devnet").is_ok());
    assert!(validate_endpoint("http://127.0.0.1:29611", "mainnet").is_ok());
    assert!(validate_endpoint("http://203.0.113.7:29611", "mainnet").is_err());
    assert!(validate_endpoint("http://nodes.example.com", "mainnet").is_err());
    assert!(validate_endpoint("https://nodes.example.com", "mainnet").is_ok());
    assert!(validate_endpoint("https://nodes.example.com", "othernet").is_err());
}
#[test]
fn signed_manifest_identity_expiry_and_tampering() {
    // Fixed test-only key, never a deployed trust anchor.
    let signer = SigningKey::from_bytes(&[7; 32]);
    let payload = BootstrapManifest {
        version: 1,
        network: "devnet".into(),
        chain_id: "test-chain".into(),
        genesis_hash: "11".repeat(32),
        issued_at: 10,
        expires_at: 100,
        endpoints: vec!["http://127.0.0.1:29411".into()],
    };
    let sig = signer.sign(&payload.signing_bytes().unwrap());
    let mut manifest = SignedManifest {
        payload,
        signature: hex::encode(sig.to_bytes()),
    };
    let key = signer.verifying_key().to_bytes();
    assert!(manifest.verify(&key, &config(vec![]), 50).is_ok());
    assert!(manifest.verify(&key, &config(vec![]), 100).is_err());
    assert!(manifest.verify(&key, &config(vec![]), 9).is_err());
    assert!(manifest.verify(&[8; 32], &config(vec![]), 50).is_err());
    manifest.payload.endpoints[0] = "http://127.0.0.2:29411".into();
    assert!(manifest.verify(&key, &config(vec![]), 50).is_err());
}
#[tokio::test]
async fn health_rejects_wrong_chain_and_height_outlier() {
    let (a, ha) = server(info(100), 200).await;
    let (b, hb) = server(info(102), 200).await;
    let (c, hc) = server(info(1_000_000), 200).await;
    let mut bad = info(100);
    bad.chain_id = "evil".into();
    let (d, hd) = server(bad, 200).await;
    let mut client = Connect::new(config(vec![a, b, c, d])).unwrap();
    let report = client.refresh().await.unwrap();
    assert_eq!(report.healthy.len(), 2);
    assert_eq!(report.rejected.len(), 2);
    assert!(
        report
            .rejected
            .iter()
            .any(|r| r.reason.contains("identity"))
    );
    assert!(report.rejected.iter().any(|r| r.reason.contains("median")));
    for handle in [ha, hb, hc, hd] {
        handle.abort();
    }
}
#[tokio::test]
async fn failover_returns_actual_api_data() {
    let (a, ha) = server(info(100), 503).await;
    let (b, hb) = server_delay(info(100), 200, 30).await;
    let mut client = Connect::new(config(vec![a.clone(), b])).unwrap();
    let body: serde_json::Value = client.get_json("/v1/supply").await.unwrap();
    assert_eq!(body["issued_supply"], "100");
    assert_eq!(client.last_report().healthy[0].endpoint, a);
    assert!(
        client
            .get_json::<serde_json::Value>("/v1/transactions")
            .await
            .is_err()
    );
    assert!(
        client
            .get_json::<serde_json::Value>("/v1/blocks?from=0&limit=10/../../admin")
            .await
            .is_err()
    );
    ha.abort();
    hb.abort();
}
#[tokio::test]
async fn no_bootstrap_fails_instead_of_inventing_a_network() {
    let mut client = Connect::new(config(vec![])).unwrap();
    assert!(client.select().await.is_err());
}
#[test]
fn unsupported_or_unpinned_configuration_rejected() {
    let mut cfg = config(vec![]);
    cfg.genesis_hash.clear();
    assert!(Connect::new(cfg).is_err());
    let mut cfg = config(vec![]);
    cfg.network = "other".into();
    assert!(Connect::new(cfg).is_err());
}

#[test]
fn block_read_query_is_bounded_and_unambiguous() {
    assert!(valid_blocks_query("/v1/blocks?from=0&limit=100"));
    for bad in [
        "/v1/blocks?limit=101",
        "/v1/blocks?limit=0",
        "/v1/blocks?from=1&from=2",
        "/v1/blocks?from=18446744073709551616",
        "/v1/blocks?limit=1%26admin=true",
    ] {
        assert!(!valid_blocks_query(bad));
    }
}
