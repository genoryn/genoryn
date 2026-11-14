//! The swap offer board on a node (`/v1/swap/...`): public offers, sealed mailboxes and
//! the market price from completed swaps (`krypton_swap::board`). Everything lives in
//! memory with hard caps and expiry, except verified trades, which are kept on disk so
//! the price survives restarts. The node never sees keys, codes or coins.

use crate::node::Node;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use krypton_core::lock::LockMode;
use krypton_swap::board::{self, Envelope, Offer, Trade};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_OFFERS: usize = 2_000;
const MAX_PER_INBOX: usize = 100;
const MAX_MESSAGES: usize = 5_000;
const MESSAGE_SECS: u64 = 24 * 3600;
const MAX_TRADES: usize = 20_000;
const TRADE_KEEP_SECS: u64 = 30 * 24 * 3600;
/// A swap in progress is shown at most this long (a GNR-first swap pays within two hours
/// of its lock and completes within a few more).
const PENDING_SECS: u64 = 12 * 3600;
/// The maker's wallet says its swap still runs every few minutes; an entry not confirmed
/// for this long is dropped (the swap was cancelled, or the wallet is gone).
const PENDING_IDLE_SECS: u64 = 30 * 60;
/// A maker seen this recently is "online" (its wallet answers takes within seconds).
const ONLINE_SECS: u64 = 120;
/// Offers of makers not seen for this long stop counting for the price.
const PRICED_SECS: u64 = 600;
/// Offers of makers not seen for this long are removed: nobody could take them.
const STALE_SECS: u64 = 30 * 60;

#[derive(Default)]
struct Inner {
    offers: HashMap<String, Offer>,
    /// Offers taken whose swaps are in progress (shown and priced, no longer takeable),
    /// with when they were taken.
    pending: HashMap<String, (Offer, u64)>,
    /// When each swap in progress was last confirmed by its maker.
    pending_seen: HashMap<String, u64>,
    /// When each offer's maker was last online (posted or pinged). Only offers whose maker
    /// is online can be taken at once, so only they count for the price.
    seen: HashMap<String, u64>,
    inbox: HashMap<String, VecDeque<(u64, String)>>,
    messages: usize,
    trades: Vec<(Trade, u64)>,
    /// Claims of locked GNR seen on chain, by hash lock, and the highest block scanned.
    claims: HashMap<String, Claim>,
    scanned: Option<u64>,
    loaded: bool,
}

#[derive(Default)]
pub struct Board {
    inner: Mutex<Inner>,
    path: Mutex<Option<PathBuf>>,
}

fn now() -> u64 {
    krypton_core::now()
}

fn bad(m: impl ToString) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": m.to_string()})),
    )
        .into_response()
}

impl Board {
    pub fn set_path(&self, p: PathBuf) {
        *self.path.lock() = Some(p);
    }

    fn expire(i: &mut Inner, t: u64) {
        i.offers
            .retain(|_, o| o.expires > t && o.sats <= board::MAX_OFFER_SATS);
        // Offers whose maker has been offline for half an hour are dropped; their makers'
        // wallets see that in their next ping and free the GNR. Any price may stay.
        let seen = &i.seen;
        i.offers
            .retain(|id, _| seen.get(id).is_some_and(|s| s + STALE_SECS > t));
        let ps = &i.pending_seen;
        i.pending.retain(|id, (_, at)| {
            *at + PENDING_SECS > t && ps.get(id).copied().unwrap_or(*at) + PENDING_IDLE_SECS > t
        });
        let pids: std::collections::HashSet<String> = i.pending.keys().cloned().collect();
        i.pending_seen.retain(|id, _| pids.contains(id));
        let ids: std::collections::HashSet<String> = i.offers.keys().cloned().collect();
        i.seen.retain(|id, _| ids.contains(id));
        let mut n = 0;
        i.inbox.retain(|_, q| {
            q.retain(|(at, _)| at + MESSAGE_SECS > t);
            n += q.len();
            !q.is_empty()
        });
        i.messages = n;
        i.trades.retain(|(_, at)| at + TRADE_KEEP_SECS > t);
    }

    fn load(&self, i: &mut Inner) {
        if i.loaded {
            return;
        }
        i.loaded = true;
        let Some(p) = self.path.lock().clone() else {
            return;
        };
        if let Ok(b) = std::fs::read(&p)
            && let Ok(t) = serde_json::from_slice::<Vec<(Trade, u64)>>(&b)
        {
            i.trades = t;
        }
        // Offers and sealed messages survive restarts too (a node update must not drop
        // them, or a swap in progress could lose a code).
        if let Ok(b) = std::fs::read(p.with_file_name("swap-board.json"))
            && let Ok(s) = serde_json::from_slice::<Saved>(&b)
        {
            i.offers = s.offers.into_iter().map(|o| (o.id.clone(), o)).collect();
            // After a restart every maker gets the usual grace to show it is still online.
            let t = now();
            i.seen = i.offers.keys().map(|id| (id.clone(), t)).collect();
            i.pending = s
                .pending
                .into_iter()
                .map(|(o, at)| (o.id.clone(), (o, at)))
                .collect();
            i.pending_seen = i.pending.keys().map(|id| (id.clone(), t)).collect();
            i.inbox = s
                .inbox
                .into_iter()
                .map(|(k, v)| (k, v.into_iter().collect()))
                .collect();
        }
        Board::expire(i, now());
    }

    fn write(path: &std::path::Path, b: Vec<u8>) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    fn save(&self, i: &Inner) {
        if let Some(p) = self.path.lock().as_ref()
            && let Ok(b) = serde_json::to_vec(&i.trades)
        {
            Board::write(p, b);
        }
    }

    fn save_board(&self, i: &Inner) {
        let Some(p) = self.path.lock().clone() else {
            return;
        };
        let s = Saved {
            offers: i.offers.values().cloned().collect(),
            pending: i.pending.values().cloned().collect(),
            inbox: i
                .inbox
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
        };
        if let Ok(b) = serde_json::to_vec(&s) {
            Board::write(&p.with_file_name("swap-board.json"), b);
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    offers: Vec<Offer>,
    #[serde(default)]
    pending: Vec<(Offer, u64)>,
    inbox: Vec<(String, Vec<(u64, String)>)>,
}

/// Scans new blocks for lock claims (their hash locks prove a swap completed). The blocks
/// are read without holding the board, which stays answerable meanwhile.
fn scan_claims(node: &Node) {
    let scanned = {
        let mut i = node.board.inner.lock();
        node.board.load(&mut i);
        i.scanned
    };
    let tip = node.chain.tip().height();
    let start = node.chain.params.lock_height.min(tip.saturating_add(1));
    let from = scanned
        .map(|s| s + 1)
        .unwrap_or(start)
        .max(tip.saturating_sub(20_000));
    let mut found = Vec::new();
    for h in from..=tip {
        if let Ok(Some(b)) = node.chain.block_at(h) {
            for t in &b.txs {
                if let Some(l) = &t.lock
                    && let LockMode::Claim { secret } = &l.mode
                {
                    found.push((
                        hex::encode(l.terms.hash),
                        Claim {
                            height: h,
                            time: b.header.timestamp,
                            txid: hex::encode(t.txid()),
                            secret: hex::encode(secret),
                            refund_height: l.terms.refund_height,
                        },
                    ));
                }
            }
        }
    }
    if from > tip {
        return;
    }
    let mut i = node.board.inner.lock();
    i.claims.extend(found);
    if i.scanned.is_none_or(|s| s < tip) {
        i.scanned = Some(tip);
    }
}

/// What the GNR chain shows of a completed swap (its amounts are private).
#[derive(Clone, serde::Serialize)]
struct Claim {
    height: u64,
    time: u64,
    txid: String,
    /// The secret revealed by the claim (the same unlocks the bitcoin side).
    secret: String,
    /// The block from which the GNR could have been refunded instead.
    refund_height: u64,
}

type S = State<Arc<Node>>;

pub async fn offers(State(node): S) -> Response {
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    Board::expire(&mut i, now());
    let t = now();
    let mut list: Vec<&Offer> = i.offers.values().collect();
    list.sort_by(|a, b| a.price().total_cmp(&b.price()));
    let v: Vec<Value> = list
        .into_iter()
        .map(|o| {
            let mut x = serde_json::to_value(o).unwrap_or_default();
            let seen = i.seen.get(&o.id).copied().unwrap_or(0);
            x["online"] = json!(seen + ONLINE_SECS > t);
            x["seen"] = json!(seen);
            x
        })
        .collect();
    let mut p: Vec<Value> = i
        .pending
        .values()
        .map(|(o, at)| json!({"id": o.id, "side": o.side, "gnr": o.gnr, "sats": o.sats, "since": at}))
        .collect();
    p.sort_by_key(|x| std::cmp::Reverse(x["since"].as_u64()));
    Json(json!({"offers": v, "in_progress": p, "now": now()})).into_response()
}

#[derive(Deserialize)]
pub struct ProgressReq {
    /// Older wallets send the whole offer; only its id is used.
    #[serde(default)]
    offer: Option<Offer>,
    #[serde(default)]
    id: Option<String>,
    secret: String,
}

/// The maker of a taken offer moves it to "in progress" (shown, no longer takeable), and
/// says every few minutes that its swap still runs. Only the offer stored on the board can
/// move, proven with its cancel secret: the request's own copy of the offer is ignored.
pub async fn post_progress(State(node): S, Json(r): Json<ProgressReq>) -> Response {
    let Some(secret) = hex::decode(&r.secret)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    else {
        return bad("bad secret");
    };
    let Some(id) = r.id.or(r.offer.map(|o| o.id)) else {
        return bad("missing offer id");
    };
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    let stored = i
        .offers
        .get(&id)
        .or_else(|| i.pending.get(&id).map(|x| &x.0));
    match stored {
        None => return bad("no such offer on the board"),
        Some(o) if o.cancel != board::cancel_hash(&secret) => return bad("wrong cancel secret"),
        Some(_) => {}
    }
    if let Some(o) = i.offers.remove(&id) {
        i.seen.remove(&id);
        i.pending.insert(id.clone(), (o, t));
    }
    i.pending_seen.insert(id, t);
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}
pub async fn post_offer(State(node): S, Json(o): Json<Offer>) -> Response {
    if let Err(e) = o.validate(node.chain.params.network.as_str(), now()) {
        return bad(e);
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    Board::expire(&mut i, now());
    if i.offers.len() >= MAX_OFFERS {
        return bad("the board is full; try again later");
    }
    if i.offers.contains_key(&o.id) {
        return bad("an offer with this id exists");
    }
    i.seen.insert(o.id.clone(), now());
    i.offers.insert(o.id.clone(), o);
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
pub struct PingItem {
    id: String,
    secret: String,
}

#[derive(Deserialize)]
pub struct PingReq {
    items: Vec<PingItem>,
}

/// The maker's wallet says it is online, for all its open offers at once (each proven
/// with its cancel secret).
pub async fn ping(State(node): S, Json(r): Json<PingReq>) -> Response {
    if r.items.len() > 500 {
        return bad("too many offers in one ping");
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    let mut ok = 0;
    let mut missing = vec![];
    for it in r.items {
        if !i.offers.contains_key(&it.id) && !i.pending.contains_key(&it.id) {
            missing.push(it.id);
            continue;
        }
        let Some(secret) = hex::decode(&it.secret)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
        else {
            continue;
        };
        let h = board::cancel_hash(&secret);
        if i.offers.get(&it.id).is_some_and(|o| o.cancel == h) {
            i.seen.insert(it.id, t);
            ok += 1;
        } else if i.pending.get(&it.id).is_some_and(|(o, _)| o.cancel == h) {
            i.pending_seen.insert(it.id, t);
            ok += 1;
        }
    }
    Json(json!({"ok": true, "online": ok, "missing": missing})).into_response()
}

#[derive(Deserialize)]
pub struct CancelReq {
    id: String,
    secret: String,
}

pub async fn cancel_offer(State(node): S, Json(r): Json<CancelReq>) -> Response {
    let Some(secret) = hex::decode(&r.secret)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    else {
        return bad("bad secret");
    };
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let found = i
        .offers
        .get(&r.id)
        .or_else(|| i.pending.get(&r.id).map(|x| &x.0));
    match found {
        Some(o) if o.cancel == board::cancel_hash(&secret) => {
            i.offers.remove(&r.id);
            i.pending.remove(&r.id);
            node.board.save_board(&i);
            Json(json!({"ok": true})).into_response()
        }
        Some(_) => bad("wrong cancel secret"),
        None => Json(json!({"ok": true, "gone": true})).into_response(),
    }
}

pub async fn post_message(State(node): S, Json(e): Json<Envelope>) -> Response {
    if let Err(x) = e.validate() {
        return bad(x);
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    if i.messages >= MAX_MESSAGES {
        return bad("the mailbox service is full; try again later");
    }
    let q = i.inbox.entry(e.to.clone()).or_default();
    if q.len() >= MAX_PER_INBOX {
        return bad("this inbox is full");
    }
    if q.iter().any(|(_, d)| *d == e.data) {
        return Json(json!({"ok": true})).into_response();
    }
    q.push_back((t, e.data));
    i.messages += 1;
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}

pub async fn inbox(State(node): S, Path(id): Path<String>) -> Response {
    if id.len() != 64 || hex::decode(&id).is_err() {
        return bad("bad inbox");
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    Board::expire(&mut i, now());
    let msgs: Vec<_> = i
        .inbox
        .get(&id)
        .map(|q| {
            q.iter()
                .map(|(at, d)| json!({"at": at, "data": d}))
                .collect()
        })
        .unwrap_or_default();
    Json(json!({"messages": msgs})).into_response()
}

pub async fn post_trade(State(node): S, Json(t): Json<Trade>) -> Response {
    if let Err(e) = t.validate(node.chain.params.network.as_str()) {
        return bad(e);
    }
    let n = node.clone();
    let r = tokio::task::spawn_blocking(move || {
        scan_claims(&n);
        let mut i = n.board.inner.lock();
        n.board.load(&mut i);
        if i.trades.iter().any(|(x, _)| x.hash == t.hash) {
            return Ok(false);
        }
        if !i.claims.contains_key(&t.hash) {
            return Err("no claim with this hash lock on chain yet");
        }
        if i.trades.len() >= MAX_TRADES {
            i.trades.remove(0);
        }
        i.trades.push((t, now()));
        n.board.save(&i);
        Ok(true)
    })
    .await;
    match r {
        Ok(Ok(added)) => Json(json!({"ok": true, "added": added})).into_response(),
        Ok(Err(e)) => bad(e),
        Err(e) => bad(e),
    }
}

/// The market now. Only offers that can be taken count: their maker was online lately.
fn current_market(i: &Inner, t: u64) -> board::Market {
    let offers: Vec<Offer> = i
        .offers
        .values()
        .filter(|o| i.seen.get(&o.id).is_some_and(|s| s + PRICED_SECS > t))
        .cloned()
        .collect();
    let pending: Vec<(Offer, u64)> = i.pending.values().cloned().collect();
    board::market(&i.trades, &offers, &pending, t)
}

pub async fn market(State(node): S) -> Response {
    // Each swap with what the GNR chain shows of it: its claim, block, time and secret.
    let n = node.clone();
    let _ = tokio::task::spawn_blocking(move || scan_claims(&n)).await;
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    let m = current_market(&i, t);
    let tip = node.chain.tip().height();
    let recent: Vec<_> = i
        .trades
        .iter()
        .rev()
        .take(50)
        .map(|(x, at)| {
            let c = i.claims.get(&x.hash);
            json!({
                "at": at, "gnr": x.gnr, "sats": x.sats, "price": x.price(), "hash": x.hash,
                "claim": c,
                "confirmations": c.map(|c| tip + 1 - c.height),
            })
        })
        .collect();
    Json(json!({"market": m, "recent": recent, "window_secs": board::PRICE_WINDOW_SECS}))
        .into_response()
}
