//! Public node API: REST (`/v1/...`), JSON-RPC 2.0 (`POST /rpc`), WebSocket (`/v1/ws`).
//! Every endpoint is read-only except transaction submission, address publishing
//! and mining work submission, all of which are fully validated.

use crate::node::{Job, Node, USER_AGENT};
use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use krypton_core::address::Address;
use krypton_core::emission::{self, subsidy};
use krypton_core::types::{Block, Transaction};
use krypton_core::{Hash32, reward};
use krypton_crypto::field::digest_to_bytes;
use krypton_store::Outcome;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

type S = State<Arc<Node>>;

struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}
fn bad(m: impl ToString) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, m.to_string())
}
fn nf(m: impl ToString) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, m.to_string())
}
fn ise(e: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
type R<T> = std::result::Result<T, ApiError>;

fn binary(v: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "application/octet-stream")], v).into_response()
}

fn parse_hash(s: &str) -> R<Hash32> {
    hex::decode(s)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| bad("expected 64 hex characters"))
}

pub fn router(node: Arc<Node>) -> Router {
    Router::new()
        .route("/v1/info", get(info))
        .route("/v1/supply", get(supply))
        .route("/v1/header/{height}", get(header_at))
        .route("/v1/block/{id}", get(block_json))
        .route("/v1/rawblock/{height}", get(raw_block))
        .route("/v1/blocks", get(blocks_list))
        .route("/v1/compact", get(compact))
        .route("/v1/frontier/{height}", get(frontier))
        .route("/v1/witness/{pos}", get(witness))
        .route("/v1/subtree/{chunk}", get(subtree))
        .route("/v1/mmrproof", get(mmr_proof))
        .route("/v1/flyclient", get(flyclient))
        .route("/v1/nullifier/{nf}", get(nullifier))
        .route("/v1/tx/{txid}", get(tx_lookup))
        .route("/v1/tx", post(submit_tx))
        .route("/v1/mempool", get(mempool))
        .route("/v1/fee", get(fee_estimate))
        .route("/v1/rawtx/{txid}", get(raw_pending_tx))
        .route("/v1/peers", get(peers))
        .route("/v1/address", post(publish_address))
        .route("/v1/address/{short}", get(resolve_address))
        .route("/v1/validate/{address}", get(validate_address))
        .route("/v1/mining/template", post(mining_template))
        .route("/v1/mining/submit", post(mining_submit))
        .route("/v1/miner/{id}", get(miner_account))
        .route("/v1/shares", get(shares_pool))
        .route("/v1/stats", get(stats))
        .route("/v1/address-info/{addr}", get(address_info))
        .route("/v1/verify-payment", post(verify_payment))
        .route("/v1/ws", get(ws))
        .route(
            "/v1/swap/offers",
            get(crate::swapboard::offers).post(crate::swapboard::post_offer),
        )
        .route(
            "/v1/swap/offers/cancel",
            post(crate::swapboard::cancel_offer),
        )
        .route("/v1/swap/progress", post(crate::swapboard::post_progress))
        .route("/v1/swap/inbox", post(crate::swapboard::post_message))
        .route("/v1/swap/inbox/{id}", get(crate::swapboard::inbox))
        .route("/v1/swap/trade", post(crate::swapboard::post_trade))
        .route("/v1/swap/market", get(crate::swapboard::market))
        .route("/rpc", post(json_rpc))
        .with_state(node.clone())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            8 * 1024 * 1024,
        ))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(axum::middleware::from_fn_with_state(
            node.guard.clone(),
            crate::guard::limit,
        ))
        // Other sites may read public data, but never drive writes from a visitor's browser.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods([axum::http::Method::GET]),
        )
}

/// Network hashrate over the last 60 blocks, estimated from the shares they credit
/// (about SHARE_RATIO samples per block, far less noisy than block times). Cached per tip.
fn hashrate(node: &Node) -> f64 {
    let tip = node.chain.tip();
    if let Some((h, v)) = *node.hashrate_cache.lock()
        && h == tip.hash
    {
        return v;
    }
    let v = estimate_hashrate(node, tip.height());
    *node.hashrate_cache.lock() = Some((tip.hash, v));
    v
}

/// Every credited share, the block's own header included, stands for `share_difficulty`
/// hashes on average.
fn estimate_hashrate(node: &Node, tip: u64) -> f64 {
    if tip < 2 {
        return 0.0;
    }
    let from = tip.saturating_sub(60).max(1);
    let (Ok(Some(a)), Ok(Some(b))) = (node.chain.header_at(from), node.chain.header_at(tip)) else {
        return 0.0;
    };
    let mut work = 0f64;
    for h in from + 1..=tip {
        if let Ok(Some(x)) = node.chain.block_at(h) {
            let share_work = reward::share_difficulty(x.header.difficulty) as f64;
            work += share_work * (x.shares.len() + 1) as f64;
        }
    }
    let dt = b.timestamp.saturating_sub(a.timestamp).max(1);
    work / dt as f64
}

fn info_value(node: &Node) -> Value {
    let tip = node.chain.tip();
    let p = &node.chain.params;
    json!({
        "network": p.network.as_str(),
        "name": p.name,
        "chain_id": p.chain_id,
        "version": USER_AGENT,
        "height": tip.height(),
        "tip": hex::encode(tip.hash),
        "genesis": hex::encode(node.chain.genesis.hash()),
        "difficulty": tip.header.difficulty.to_string(),
        "cumulative_work": tip.work.to_string(),
        "timestamp": tip.header.timestamp,
        "supply": node.chain.supply(),
        "max_supply": emission::MAX_SUPPLY,
        "next_subsidy": subsidy(tip.height() + 1),
        "note_count": tip.frontier.size,
        "note_root": hex::encode(tip.header.note_root),
        "peers": node.net.ready_peers().len(),
        "best_peer_height": node.best_peer_height(),
        "mempool": node.mempool.lock().txs.len(),
        "hashrate": hashrate(node),
        "reward_pool": node.chain.pool().unwrap_or(0),
        "stats": node.stats_summary.lock().clone(),
        "shares_pending": node.shares.lock().shares.len(),
        "share_ratio": krypton_core::params::SHARE_RATIO as u64,
        "share_difficulty": reward::share_difficulty(tip.header.difficulty).to_string(),
        "payout_interval": p.payout_interval,
        "coinbase_maturity": p.coinbase_maturity,
        "uptime": node.started.elapsed().as_secs(),
        "launch_anchor": p.launch_anchor.map(|a| json!({"bitcoin_height": a.bitcoin_height, "bitcoin_hash": a.bitcoin_hash})),
        "crypto": {
            "spend_proofs": "zk-STARK (Winterfell, Rescue-Prime, SHA3 commitments)",
            "note_encryption": "X25519 + ML-KEM-1024 (FIPS 203), XChaCha20-Poly1305",
            "p2p": "X25519 + ML-KEM-768, ChaCha20-Poly1305",
            "pow": "KryptoHash v1",
        }
    })
}

async fn info(State(n): S) -> Json<Value> {
    Json(info_value(&n))
}

async fn supply(State(n): S) -> Json<Value> {
    let h = n.chain.tip().height();
    Json(json!({
        "height": h,
        "issued": n.chain.supply(),
        "issued_gnr": emission::format_amount(n.chain.supply()),
        "max_supply": emission::MAX_SUPPLY,
        "subsidy": subsidy(h + 1),
        "halving_interval": emission::HALVING_INTERVAL,
        "next_halving_height": (h / emission::HALVING_INTERVAL + 1) * emission::HALVING_INTERVAL + 1,
    }))
}

async fn header_at(State(n): S, Path(h): Path<u64>) -> R<Json<Value>> {
    let hd = n
        .chain
        .header_at(h)
        .map_err(ise)?
        .ok_or_else(|| nf("no such height"))?;
    let mut v = serde_json::to_value(&hd).map_err(ise)?;
    v["hash"] = json!(hex::encode(hd.hash()));
    Ok(Json(v))
}

fn fees(b: &Block) -> u64 {
    b.txs.iter().map(|t| t.fee).sum()
}

fn block_summary(b: &Block, payouts: &[reward::Payout], confirmations: u64) -> Value {
    let txs: Vec<Value> = b
        .txs
        .iter()
        .map(|t| {
            json!({
                "txid": hex::encode(t.txid()),
                "kind": t.kind,
                "fee": t.fee,
                "size": t.size(),
                "nullifiers": t.nullifiers.iter().map(|x| hex::encode(x.0)).collect::<Vec<_>>(),
                "commitments": t.outputs.iter().map(|o| hex::encode(o.cm)).collect::<Vec<_>>(),
                "proof_bytes": t.proof.len(),
            })
        })
        .collect();
    json!({
        "hash": hex::encode(b.hash()),
        "header": b.header,
        "size": b.to_bytes().len(),
        "confirmations": confirmations,
        "subsidy": subsidy(b.header.height),
        "fees": fees(b),
        "miner": hex::encode(b.header.miner),
        "shares": b.shares.iter().map(|s| json!({"hash": hex::encode(s.hash()), "height": s.height, "miner": hex::encode(s.miner)})).collect::<Vec<_>>(),
        "payouts": payouts,
        "txs": txs,
    })
}

async fn block_json(State(n): S, Path(id): Path<String>) -> R<Json<Value>> {
    let b = if id.len() == 64 {
        n.chain.block(&parse_hash(&id)?).map_err(ise)?
    } else {
        n.chain.block_at(id.parse().map_err(bad)?).map_err(ise)?
    }
    .ok_or_else(|| nf("block not found"))?;
    let tip = n.chain.tip().height();
    let main = n.chain.is_main(&b.hash()).map_err(ise)?;
    let conf = if main {
        (tip + 1).saturating_sub(b.header.height)
    } else {
        0
    };
    let payouts = if main {
        n.chain.payouts_at(b.header.height).map_err(ise)?
    } else {
        vec![]
    };
    Ok(Json(block_summary(&b, &payouts, conf)))
}

async fn raw_block(State(n): S, Path(h): Path<u64>) -> R<Response> {
    let b = n
        .chain
        .block_at(h)
        .map_err(ise)?
        .ok_or_else(|| nf("no such height"))?;
    Ok(binary(b.to_bytes()))
}

#[derive(Deserialize)]
struct Range {
    from: Option<u64>,
    count: Option<u32>,
}

async fn blocks_list(State(n): S, Query(q): Query<Range>) -> R<Json<Value>> {
    let tip = n.chain.tip().height();
    let count = q.count.unwrap_or(20).min(100) as u64;
    let from = q.from.unwrap_or(tip);
    let chain = n.chain.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
        let mut v = Vec::new();
        let mut h = from.min(tip) as i64;
        while h >= 0 && (v.len() as u64) < count {
            if let Some(b) = chain.block_at(h as u64)? {
                v.push(json!({
                    "height": b.header.height,
                    "hash": hex::encode(b.hash()),
                    "timestamp": b.header.timestamp,
                    "difficulty": b.header.difficulty.to_string(),
                    "txs": b.txs.len(),
                    "shares": b.shares.len(),
                    "size": b.to_bytes().len(),
                    "subsidy": subsidy(b.header.height),
                    "fees": fees(&b),
                    "miner": hex::encode(b.header.miner),
                }));
            }
            h -= 1;
        }
        Ok(v)
    })
    .await
    .map_err(ise)?
    .map_err(ise)?;
    Ok(Json(json!(v)))
}

async fn compact(State(n): S, Query(q): Query<Range>) -> R<Response> {
    let from = q.from.ok_or_else(|| bad("from required"))?;
    let count = q.count.unwrap_or(50).min(200) as u64;
    let chain = n.chain.clone();
    let list = tokio::task::spawn_blocking(
        move || -> anyhow::Result<Vec<krypton_light::CompactBlock>> {
            // Bounded response: wallets continue from the last block they received.
            const MAX_BYTES: usize = 16 * 1024 * 1024;
            let mut v = Vec::new();
            let mut bytes = 0usize;
            for h in from..from.saturating_add(count) {
                if bytes >= MAX_BYTES {
                    break;
                }
                match chain.compact_block(h)? {
                    Some(b) => {
                        bytes += b.outputs.len() * (36 + krypton_core::types::NOTE_CIPHERTEXT_LEN)
                            + b.nullifiers.len() * 32
                            + 512;
                        v.push(b);
                    }
                    None => break,
                }
            }
            Ok(v)
        },
    )
    .await
    .map_err(ise)?
    .map_err(ise)?;
    Ok(binary(krypton_light::encode_compact_list(&list)))
}

async fn frontier(State(n): S, Path(h): Path<u64>) -> R<Response> {
    let f = n
        .chain
        .frontier_at(h)
        .map_err(ise)?
        .ok_or_else(|| nf("frontiers are kept every 64 blocks and at the tip"))?;
    Ok(binary(f.to_bytes()))
}

async fn witness(State(n): S, Path(pos): Path<u64>) -> R<Json<Value>> {
    let (path, root) = n.chain.auth_path(pos).map_err(bad)?;
    Ok(Json(json!({
        "path": path.iter().map(|d| hex::encode(digest_to_bytes(d))).collect::<Vec<_>>(),
        "root": hex::encode(digest_to_bytes(&root)),
    })))
}

async fn subtree(State(n): S, Path(chunk): Path<u64>) -> R<Json<Value>> {
    const L: u8 = 12;
    let size = n.chain.tip().frontier.size;
    // Compare chunk counts, not shifted indices: `chunk << L` wraps for huge chunks.
    if chunk >= size.div_ceil(1 << L) {
        return Err(nf("chunk beyond the tree"));
    }
    let chain = n.chain.clone();
    let (leaves, upper, root) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let leaves = chain.leaves(chunk << L, 1 << L)?;
        let mut upper = Vec::new();
        for level in L..krypton_zk::layout::DEPTH as u8 {
            let idx = (chunk >> (level - L)) ^ 1;
            upper.push(hex::encode(digest_to_bytes(&chain.tree_node(level, idx)?)));
        }
        Ok((leaves, upper, chain.note_root_digest()))
    })
    .await
    .map_err(ise)?
    .map_err(ise)?;
    Ok(Json(json!({
        "leaves": leaves.iter().map(hex::encode).collect::<Vec<_>>(),
        "upper": upper,
        "root": hex::encode(digest_to_bytes(&root)),
    })))
}

#[derive(Deserialize)]
struct MmrQ {
    leaf: u64,
    at: Option<u64>,
}

async fn mmr_proof(State(n): S, Query(q): Query<MmrQ>) -> R<Json<Value>> {
    let tip = n.chain.tip().height();
    let at = q.at.unwrap_or(tip);
    if at > tip {
        return Err(nf("leaf not committed at that height"));
    }
    let chain = n.chain.clone();
    let p = tokio::task::spawn_blocking(move || chain.mmr_proof(q.leaf, at))
        .await
        .map_err(ise)?
        .map_err(ise)?
        .ok_or_else(|| nf("leaf not committed at that height"))?;
    Ok(Json(serde_json::to_value(p).map_err(ise)?))
}

#[derive(Deserialize)]
struct FlyQ {
    heights: Option<String>,
}

async fn flyclient(State(n): S, Query(q): Query<FlyQ>) -> R<Json<Value>> {
    let tip = n.chain.tip();
    let heights: Vec<u64> = q
        .heights
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().map_err(bad))
        .collect::<R<_>>()?;
    if heights.len() > 200 {
        return Err(bad("at most 200 samples"));
    }
    let chain = n.chain.clone();
    let fp = tokio::task::spawn_blocking(move || -> R<krypton_light::FlyProof> {
        let mut samples = Vec::new();
        for h in heights {
            if h >= tip.height() {
                continue;
            }
            let header = chain
                .header_at(h)
                .map_err(ise)?
                .ok_or_else(|| nf("height"))?;
            let proof = chain
                .mmr_proof(h, tip.height())
                .map_err(ise)?
                .ok_or_else(|| nf("proof"))?;
            samples.push(krypton_light::FlySample { header, proof });
        }
        let mut recent = Vec::new();
        let start = tip
            .height()
            .saturating_sub(krypton_core::difficulty::WINDOW as u64 + 10);
        for h in start..tip.height() {
            recent.push(
                chain
                    .header_at(h)
                    .map_err(ise)?
                    .ok_or_else(|| nf("height"))?,
            );
        }
        Ok(krypton_light::FlyProof {
            tip: tip.header.clone(),
            recent,
            samples,
        })
    })
    .await
    .map_err(ise)??;
    Ok(Json(serde_json::to_value(fp).map_err(ise)?))
}

async fn nullifier(State(n): S, Path(x): Path<String>) -> R<Json<Value>> {
    let h = parse_hash(&x)?;
    Ok(Json(
        json!({ "spent_height": n.chain.nullifier_height(&h).map_err(ise)? }),
    ))
}

async fn tx_lookup(State(n): S, Path(x): Path<String>) -> R<Json<Value>> {
    let id = parse_hash(&x)?;
    if let Some(e) = n.mempool.lock().txs.get(&id) {
        return Ok(Json(
            json!({"txid": x, "status": "pending", "fee": e.tx.fee, "size": e.size}),
        ));
    }
    let h = n
        .chain
        .tx_height(&id)
        .map_err(ise)?
        .ok_or_else(|| nf("transaction not found"))?;
    let tip = n.chain.tip().height();
    Ok(Json(
        json!({"txid": x, "status": "confirmed", "height": h, "confirmations": (tip + 1).saturating_sub(h)}),
    ))
}

async fn submit_tx(State(n): S, body: Bytes) -> R<Json<Value>> {
    let tx = Transaction::from_bytes(&body).map_err(bad)?;
    let busy = || {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy verifying transactions; retry in a moment".into(),
        )
    };
    let _verifying = tokio::time::timeout(Duration::from_secs(10), n.tx_verify.acquire())
        .await
        .map_err(|_| busy())?
        .map_err(|_| busy())?;
    let id = n.accept_tx(tx).await.map_err(bad)?;
    n.relay_local(id);
    Ok(Json(json!({ "txid": hex::encode(id) })))
}

/// A pending (mempool) transaction in its binary encoding, so wallets can see incoming
/// payments and check payment proofs before the next block. Confirmed transactions are
/// part of their block (`/v1/rawblock`).
async fn raw_pending_tx(State(n): S, Path(x): Path<String>) -> R<Response> {
    let id = parse_hash(&x)?;
    let bytes = n
        .mempool
        .lock()
        .txs
        .get(&id)
        .map(|e| e.tx.to_bytes())
        .ok_or_else(|| nf("not in the mempool"))?;
    Ok(binary(bytes))
}

/// Fee suggestions per KB (a transfer is about 100 KB): mined within about 1, 3 and 12
/// blocks given the current backlog. Fees are a market; these are only estimates.
async fn fee_estimate(State(n): S) -> Json<Value> {
    use krypton_core::params::{FEE_ESTIMATE_KB, MAX_BLOCK_BYTES, MIN_FEE_PER_KB};
    let room = MAX_BLOCK_BYTES - 512 * 1024;
    let mp = n.mempool.lock();
    let (fast, normal, economy) = (
        mp.fee_per_kb_within(1, room),
        mp.fee_per_kb_within(3, room),
        mp.fee_per_kb_within(12, room),
    );
    Json(json!({
        "min_per_kb": MIN_FEE_PER_KB,
        "economy_per_kb": economy,
        "normal_per_kb": normal,
        "fast_per_kb": fast,
        "transfer_kb": FEE_ESTIMATE_KB,
        "mempool_bytes": mp.bytes(),
        "mempool_txs": mp.txs.len(),
    }))
}

async fn mempool(State(n): S) -> Json<Value> {
    let mp = n.mempool.lock();
    let mut txs: Vec<Value> = mp
        .txs
        .iter()
        .map(|(id, e)| json!({"txid": hex::encode(id), "fee": e.tx.fee, "size": e.size, "added": e.added}))
        .collect();
    txs.truncate(500);
    Json(json!({"count": mp.txs.len(), "bytes": mp.bytes(), "txs": txs}))
}

async fn peers(State(n): S) -> Json<Value> {
    let v: Vec<Value> = n
        .net
        .peers()
        .iter()
        .filter_map(|p| {
            let h = p.hello.as_ref()?;
            Some(json!({"inbound": p.inbound, "height": h.height, "agent": h.user_agent, "since": p.since}))
        })
        .collect();
    Json(json!(v))
}

async fn publish_address(State(n): S, body: String) -> R<Json<Value>> {
    let a = Address::decode(&body).map_err(bad)?;
    if a.network != n.chain.params.network {
        return Err(bad("address is for another network"));
    }
    let id = a.short_id();
    if n.chain.get_address(&id).map_err(ise)?.is_none() {
        // Entries are content-addressed and permanent: bound how fast anyone can add them.
        {
            let mut b = n.address_budget.lock();
            let now = Instant::now();
            b.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            if b.len() >= 120 {
                return Err(ApiError(
                    StatusCode::TOO_MANY_REQUESTS,
                    "address directory is busy; try again in a minute".into(),
                ));
            }
            b.push(now);
        }
        n.chain
            .put_address(&id, a.encode().as_bytes())
            .map_err(ise)?;
    }
    Ok(Json(json!({"short": a.short()})))
}

async fn resolve_address(State(n): S, Path(short): Path<String>) -> R<Json<Value>> {
    let (_, id) = Address::parse_short(&short).map_err(bad)?;
    let full = n
        .chain
        .get_address(&id)
        .map_err(ise)?
        .ok_or_else(|| nf("unknown short address"))?;
    let s = String::from_utf8(full).map_err(ise)?;
    // Content-addressed: verify before returning.
    let a = Address::decode(&s).map_err(ise)?;
    if a.short_id() != id {
        return Err(ise("directory entry mismatch"));
    }
    Ok(Json(json!({"address": s})))
}

async fn validate_address(State(n): S, Path(a): Path<String>) -> Json<Value> {
    match Address::decode(&a) {
        Ok(x) => Json(
            json!({"valid": x.network == n.chain.params.network, "network": x.network.as_str(), "short": x.short()}),
        ),
        Err(e) => Json(json!({"valid": false, "error": e.to_string()})),
    }
}

#[derive(Deserialize)]
struct TemplateReq {
    address: String,
}

async fn template_for(n: &Arc<Node>, address: &str) -> R<Value> {
    let addr = Address::decode(address).map_err(bad)?;
    if addr.network != n.chain.params.network {
        return Err(bad("payout address is for another network"));
    }
    let owner = digest_to_bytes(&addr.owner);
    let key = hex::encode(owner);
    let tip = n.chain.tip();
    let rev = n.mempool.lock().revision ^ (n.shares.lock().revision << 32);
    if let Some((hash, r, at, job)) = n.templates.lock().get(&key).copied()
        && hash == tip.hash
        && (r == rev || at.elapsed() < Duration::from_secs(10))
        && let Some(v) = job_json(n, job)
    {
        return Ok(v);
    }
    {
        // Bound template building on public endpoints.
        let mut b = n.template_budget.lock();
        let now = Instant::now();
        b.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        if b.len() >= 600 {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "template rate limit; reuse your payout address".into(),
            ));
        }
        b.push(now);
    }
    let txs = n
        .mempool
        .lock()
        .select(krypton_core::params::MAX_BLOCK_BYTES - 512 * 1024);
    let shares = n.shares.lock().for_template();
    let chain = n.chain.clone();
    let block = tokio::task::spawn_blocking(move || -> anyhow::Result<Block> {
        chain.template(owner, shares, txs, krypton_core::now())
    })
    .await
    .map_err(ise)?
    .map_err(ise)?;
    let id: u64 = rand::random::<u64>() >> 12;
    {
        // Every job holds a full block: bound their memory (oldest go first; miners
        // refresh their template every few seconds).
        const MAX_JOB_BYTES: usize = 256 * 1024 * 1024;
        let size = block.to_bytes().len();
        let mut jobs = n.jobs.lock();
        let mut bytes: usize = jobs.values().map(|j| j.bytes).sum::<usize>() + size;
        while bytes > MAX_JOB_BYTES || jobs.len() >= 20_000 {
            let Some(oldest) = jobs.iter().min_by_key(|(_, j)| j.created).map(|(k, _)| *k) else {
                break;
            };
            if let Some(j) = jobs.remove(&oldest) {
                bytes -= j.bytes;
            }
        }
        jobs.insert(
            id,
            Job {
                block,
                created: Instant::now(),
                bytes: size,
            },
        );
    }
    {
        let mut t = n.templates.lock();
        // Entries for older tips can never be reused.
        t.retain(|_, e| e.0 == tip.hash);
        t.insert(key, (tip.hash, rev, Instant::now(), id));
    }
    job_json(n, id).ok_or_else(|| ise("job expired"))
}

fn job_json(n: &Node, id: u64) -> Option<Value> {
    let jobs = n.jobs.lock();
    let b = &jobs.get(&id)?.block;
    let h = &b.header;
    let target = krypton_pow::target_from_difficulty(h.difficulty);
    let share_difficulty = reward::share_difficulty(h.difficulty);
    let share_target = krypton_pow::target_from_difficulty(share_difficulty);
    let epoch = krypton_pow::epoch_of(h.height);
    let hex_target = |t: &[u64; 4]| t.iter().map(|l| format!("{l:016x}")).collect::<Vec<_>>();
    Some(json!({
        "job_id": id,
        "height": h.height,
        "pow_hash": hex::encode(h.pow_hash()),
        "difficulty": h.difficulty.to_string(),
        "target": hex_target(&target),
        "share_difficulty": share_difficulty.to_string(),
        "share_target": hex_target(&share_target),
        "epoch": epoch,
        "chain_seed": hex::encode(n.chain.genesis.hash()),
        "network": n.chain.params.network.as_str(),
        "subsidy": subsidy(h.height),
        "shares": b.shares.len(),
        "txs": b.txs.len(),
    }))
}

async fn mining_template(State(n): S, Json(r): Json<TemplateReq>) -> R<Json<Value>> {
    Ok(Json(template_for(&n, &r.address).await?))
}

#[derive(Deserialize)]
struct SubmitReq {
    job_id: u64,
    nonce: u64,
}

/// Accepts a solution for a job: a full block when it meets the block target,
/// otherwise a share when it meets the share target.
async fn mining_submit(State(n): S, Json(r): Json<SubmitReq>) -> R<Json<Value>> {
    // Only the header is copied here; the (up to 4 MiB) body only for a real block.
    let mut h = n
        .jobs
        .lock()
        .get(&r.job_id)
        .map(|j| j.block.header.clone())
        .ok_or_else(|| nf("unknown or expired job"))?;
    h.nonce = r.nonce;
    let ph = h.pow_hash();
    let (meets_block, meets_share) = {
        let (chain, height, diff, nonce) = (n.chain.clone(), h.height, h.difficulty, r.nonce);
        tokio::task::spawn_blocking(move || {
            let v = chain.pow.pow_hash(height, &ph, nonce);
            (
                krypton_pow::meets_difficulty(&v, diff),
                krypton_pow::meets_difficulty(&v, reward::share_difficulty(diff)),
            )
        })
        .await
        .map_err(ise)?
    };
    let as_share = |n: Arc<Node>, h: krypton_core::BlockHeader| async move {
        match n.accept_share(h.clone(), None).await {
            Ok(_) => Ok(Json(json!({
                "accepted": true, "kind": "share", "height": h.height, "hash": hex::encode(h.hash()),
            }))),
            Err(e) => Ok(Json(
                json!({"accepted": false, "kind": "share", "reason": e.to_string()}),
            )),
        }
    };
    if meets_block {
        let mut block = n
            .jobs
            .lock()
            .get(&r.job_id)
            .map(|j| j.block.clone())
            .ok_or_else(|| nf("unknown or expired job"))?;
        block.header.nonce = r.nonce;
        return match n.submit_block(block.clone()).await {
            Ok(Outcome::Connected { .. }) => {
                tracing::info!(height = h.height, "mined block accepted");
                Ok(Json(json!({
                    "accepted": true, "kind": "block", "height": h.height, "hash": hex::encode(block.hash()),
                })))
            }
            // A block that lost a race still earns as a share.
            Ok(_) => as_share(n.clone(), h).await,
            Err(e) => Err(bad(e)),
        };
    }
    if !meets_share {
        return Err(bad("solution does not meet the share target"));
    }
    as_share(n.clone(), h).await
}

/// Owner bytes from a full address or a 64-hex owner.
fn parse_owner(n: &Node, s: &str) -> R<Hash32> {
    if s.len() == 64 {
        return parse_hash(s);
    }
    let a = Address::decode(s).map_err(bad)?;
    if a.network != n.chain.params.network {
        return Err(bad("address is for another network"));
    }
    Ok(digest_to_bytes(&a.owner))
}

/// Unpaid mining rewards of an owner and its recent shares.
async fn miner_account(State(n): S, Path(id): Path<String>) -> R<Json<Value>> {
    let owner = parse_owner(&n, &id)?;
    let chain = n.chain.clone();
    let tip = chain.tip();
    let interval = chain.params.payout_interval;
    let account = chain.account(&owner).map_err(ise)?;
    let cached = n
        .recent_shares
        .lock()
        .as_ref()
        .filter(|(h, _, _)| *h == tip.hash)
        .map(|(_, m, b)| (m.get(&owner).copied().unwrap_or(0), *b));
    let (recent, blocks_seen) = match cached {
        Some(x) => x,
        None => {
            let height = tip.height();
            let counts = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let from = height.saturating_sub(59).max(1);
                let mut m: std::collections::HashMap<Hash32, u64> = Default::default();
                for h in from..=height {
                    if let Some(b) = chain.block_at(h)? {
                        *m.entry(b.header.miner).or_default() += 1;
                        for s in &b.shares {
                            *m.entry(s.miner).or_default() += 1;
                        }
                    }
                }
                Ok((m, height + 1 - from.min(height + 1)))
            })
            .await
            .map_err(ise)?
            .map_err(ise)?;
            let r = (counts.0.get(&owner).copied().unwrap_or(0), counts.1);
            *n.recent_shares.lock() = Some((tip.hash, counts.0, counts.1));
            r
        }
    };
    let tip = tip.height();
    let pending = n
        .shares
        .lock()
        .shares
        .values()
        .filter(|(s, _)| s.miner == owner)
        .count();
    Ok(Json(json!({
        "owner": hex::encode(owner),
        "unpaid": account.map(|a| a.amount).unwrap_or(0),
        "since": account.map(|a| a.since),
        "next_payout_height": account.map(|a| a.due_height(interval)),
        "payout_interval": interval,
        "shares_recent": recent,
        "shares_recent_blocks": blocks_seen,
        "shares_pending": pending,
        "network_shares_per_block": krypton_core::params::SHARE_RATIO as u64,
        "height": tip,
    })))
}

/// Everything public about an address: validity, short form, directory entry and
/// mining activity. Transfers are shielded and never appear here.
async fn address_info(State(n): S, Path(a): Path<String>) -> R<Json<Value>> {
    let a = a.trim().to_string();
    let net = &n.chain.params;
    let (full, short, published) = if a.contains("s1") && a.len() < 120 {
        let (network, id) = Address::parse_short(&a).map_err(bad)?;
        if network != net.network {
            return Err(bad(format!("short address for {}", network.as_str())));
        }
        match n.chain.get_address(&id).map_err(ise)? {
            Some(f) => (Some(String::from_utf8(f).map_err(ise)?), a.clone(), true),
            None => (None, a.clone(), false),
        }
    } else {
        let addr = Address::decode(&a).map_err(bad)?;
        if addr.network != net.network {
            return Err(bad(format!("address for {}", addr.network.as_str())));
        }
        let published = n
            .chain
            .get_address(&addr.short_id())
            .map_err(ise)?
            .is_some();
        (Some(a.clone()), addr.short(), published)
    };
    let Some(full) = full else {
        return Ok(Json(json!({
            "valid": true, "network": net.network.as_str(), "kind": "short", "short": short,
            "published": false,
        })));
    };
    let addr = Address::decode(&full).map_err(ise)?;
    let owner = digest_to_bytes(&addr.owner);
    let account = n.chain.account(&owner).map_err(ise)?;
    let (chain, st) = (n.chain.clone(), n.stats.clone());
    let activity = tokio::task::spawn_blocking(move || {
        let mut s = st.lock();
        s.refresh(&chain);
        s.activity(&owner)
    })
    .await
    .map_err(ise)?;
    Ok(Json(json!({
        "valid": true,
        "network": net.network.as_str(),
        "kind": "full",
        "short": short,
        "published": published,
        "owner": hex::encode(owner),
        "crypto": "ML-KEM-1024 + X25519 key, 128-bit checksum",
        "height": n.chain.tip().height(),
        "mining": {
            "unpaid": account.map(|x| x.amount).unwrap_or(0),
            "next_payout_height": account.map(|x| x.due_height(net.payout_interval)),
            "maturity": net.coinbase_maturity,
            "activity": activity,
        },
    })))
}

/// The node's own chain as a proof source (it validated every block itself), plus the
/// transaction from its mempool when the proof is about a pending payment.
struct ChainProof(Arc<krypton_store::Chain>, Option<Transaction>);
impl krypton_wallet_core::proof::ProofSource for ChainProof {
    fn tx_height(&self, txid: &Hash32) -> anyhow::Result<Option<u64>> {
        self.0.tx_height(txid)
    }
    fn raw_block(&self, height: u64) -> anyhow::Result<Block> {
        self.0
            .block_at(height)?
            .ok_or_else(|| anyhow::anyhow!("no block"))
    }
    fn proven_header(
        &self,
        height: u64,
        _: &krypton_core::ChainParams,
        _: &krypton_pow::Verifier,
    ) -> anyhow::Result<krypton_core::BlockHeader> {
        self.0
            .header_at(height)?
            .ok_or_else(|| anyhow::anyhow!("no header"))
    }
    fn tip_height(&self) -> anyhow::Result<u64> {
        Ok(self.0.tip().height())
    }
    fn pending_tx(&self, txid: &Hash32) -> anyhow::Result<Option<Transaction>> {
        Ok(self.1.clone().filter(|t| t.txid() == *txid))
    }
    fn resolve_recipient(&self, short: &str) -> anyhow::Result<Option<String>> {
        let (_, id) = Address::parse_short(short)?;
        Ok(self
            .0
            .get_address(&id)?
            .and_then(|b| String::from_utf8(b).ok()))
    }
}

#[derive(Deserialize)]
struct ProofReq {
    proof: String,
    address: Option<String>,
}

/// Checks a payment proof (`gnrpay1…`) against this node's chain.
async fn verify_payment(State(n): S, Json(r): Json<ProofReq>) -> R<Json<Value>> {
    let p = krypton_wallet_core::proof::PaymentProof::decode(&r.proof).map_err(bad)?;
    if p.network != n.chain.params.network {
        return Err(bad("proof is for another network"));
    }
    let recipient = match r
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(a) => Some(Address::decode(a).map_err(bad)?),
        None => None,
    };
    let chain = n.chain.clone();
    let pending = n.mempool.lock().txs.get(&p.txid).map(|e| e.tx.clone());
    let rep = tokio::task::spawn_blocking(move || {
        krypton_wallet_core::proof::verify(
            &p,
            &ChainProof(chain.clone(), pending),
            recipient.as_ref(),
            &chain.pow,
        )
    })
    .await
    .map_err(ise)?
    .map_err(bad)?;
    Ok(Json(serde_json::to_value(rep).map_err(ise)?))
}

#[derive(Deserialize)]
struct StatsQ {
    range: Option<String>,
}

/// Time series for charts: `range` = 1h, 6h, 24h, 7d, 30d or all.
async fn stats(State(n): S, Query(q): Query<StatsQ>) -> R<Json<Value>> {
    let range = q.range.unwrap_or_else(|| "24h".into());
    let seconds = match range.as_str() {
        "1h" => 3_600,
        "6h" => 21_600,
        "24h" => 86_400,
        "7d" => 604_800,
        "30d" => 2_592_000,
        "all" => 0,
        _ => return Err(bad("range must be 1h, 6h, 24h, 7d, 30d or all")),
    };
    let (chain, st) = (n.chain.clone(), n.stats.clone());
    let v = tokio::task::spawn_blocking(move || {
        let mut s = st.lock();
        s.refresh(&chain);
        json!({ "range": range, "summary": s.summary(), "points": s.series(seconds) })
    })
    .await
    .map_err(ise)?;
    Ok(Json(v))
}

async fn shares_pool(State(n): S) -> Json<Value> {
    let sp = n.shares.lock();
    let recent: Vec<Value> = sp
        .recent(50)
        .iter()
        .map(|s| json!({"hash": hex::encode(s.hash()), "height": s.height, "miner": hex::encode(s.miner)}))
        .collect();
    Json(json!({
        "pending": sp.shares.len(),
        "revision": sp.revision,
        "share_ratio": krypton_core::params::SHARE_RATIO as u64,
        "share_window": krypton_core::params::SHARE_WINDOW,
        "recent": recent,
    }))
}

async fn ws(State(n): S, up: WebSocketUpgrade) -> Response {
    let Ok(permit) = n.guard.ws.clone().try_acquire_owned() else {
        return ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many subscribers".into(),
        )
        .into_response();
    };
    // Subscribers only receive; anything they send is tiny.
    up.max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |s| async move {
            ws_loop(n, s).await;
            drop(permit);
        })
}

async fn ws_loop(n: Arc<Node>, mut s: WebSocket) {
    // A subscriber that stops reading is dropped instead of holding a task forever.
    const SEND: Duration = Duration::from_secs(10);
    let mut rx = n.events.subscribe();
    let first = WsMessage::Text(info_value(&n).to_string().into());
    if !matches!(tokio::time::timeout(SEND, s.send(first)).await, Ok(Ok(()))) {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(e) => if !matches!(tokio::time::timeout(SEND, s.send(WsMessage::Text(e.into()))).await, Ok(Ok(()))) { break },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            m = s.recv() => if m.is_none() { break },
        }
    }
}

/// JSON-RPC 2.0 facade over the same handlers (familiar names for integrators).
async fn json_rpc(State(n): S, _h: HeaderMap, Json(req): Json<Value>) -> Json<Value> {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!([]));
    let p0 = params.get(0).cloned().unwrap_or(Value::Null);
    let result: R<Value> = async {
        Ok(match method {
            "getinfo" | "getblockchaininfo" => info_value(&n),
            "getblockcount" => json!(n.chain.tip().height()),
            "getbestblockhash" => json!(hex::encode(n.chain.tip().hash)),
            "getblockhash" => {
                let h = p0.as_u64().ok_or_else(|| bad("height"))?;
                json!(n.chain.main_hash(h).map_err(ise)?.map(hex::encode))
            }
            "getblock" => {
                let s = p0
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| p0.as_u64().map(|h| h.to_string()))
                    .ok_or_else(|| bad("id"))?;
                block_json(State(n.clone()), Path(s)).await?.0
            }
            "getblockheader" => {
                header_at(
                    State(n.clone()),
                    Path(p0.as_u64().ok_or_else(|| bad("height"))?),
                )
                .await?
                .0
            }
            "getsupply" => supply(State(n.clone())).await.0,
            "getmempoolinfo" => mempool(State(n.clone())).await.0,
            "getpeerinfo" => peers(State(n.clone())).await.0,
            "getnetworkhashps" => json!(hashrate(&n)),
            "sendrawtransaction" => {
                let raw = hex::decode(p0.as_str().ok_or_else(|| bad("hex tx"))?).map_err(bad)?;
                submit_tx(State(n.clone()), Bytes::from(raw)).await?.0
            }
            "gettransaction" => {
                tx_lookup(
                    State(n.clone()),
                    Path(p0.as_str().unwrap_or("").to_string()),
                )
                .await?
                .0
            }
            "validateaddress" => {
                validate_address(
                    State(n.clone()),
                    Path(p0.as_str().unwrap_or("").to_string()),
                )
                .await
                .0
            }
            "resolveaddress" => {
                resolve_address(
                    State(n.clone()),
                    Path(p0.as_str().unwrap_or("").to_string()),
                )
                .await?
                .0
            }
            "getblocktemplate" => {
                template_for(&n, p0.as_str().ok_or_else(|| bad("payout address"))?).await?
            }
            "submitwork" => {
                let job_id = params
                    .get(0)
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| bad("job_id"))?;
                let nonce = params
                    .get(1)
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| bad("nonce"))?;
                mining_submit(State(n.clone()), Json(SubmitReq { job_id, nonce }))
                    .await?
                    .0
            }
            _ => {
                return Err(ApiError(
                    StatusCode::NOT_FOUND,
                    format!("unknown method {method}"),
                ));
            }
        })
    }
    .await;
    Json(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(ApiError(c, m)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -(c.as_u16() as i64), "message": m}})
        }
    })
}
