//! The swap offer board on a node (`/v1/swap/...`): public offers, sealed mailboxes and
//! the market price from completed swaps (`krypton_swap::board`). Everything lives in
//! memory with hard caps and expiry, except verified trades, which are kept on disk so
//! the price survives restarts. The node never sees keys, codes or coins.

use crate::node::Node;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use krypton_core::Hash32;
use krypton_core::consensus::StateView;
use krypton_core::lock::LockMode;
use krypton_core::types::Transaction;
use krypton_swap::board::{self, Backing, CoinOffer, CoinTrade, Envelope, Offer, Side, Trade};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_OFFERS: usize = 5_000;
const MAX_PER_INBOX: usize = 100;
const MAX_MESSAGES: usize = 5_000;
/// All stored messages together (coin swaps send ~200 KB ones).
const MAX_MESSAGE_BYTES_TOTAL: usize = 96 * 1024 * 1024;
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
/// Offers of makers not seen for this long are removed. Until then an offline maker's
/// offer stays listed (greyed out, not takeable), so an offer for a week or a year
/// survives a computer that sleeps at night.
const STALE_SECS: u64 = 7 * 24 * 3600;
/// The rules the stored offers were accepted under. 2: every sell offer proves its GNR
/// (`krypton_swap::board::check_reserve`). Offers stored under older rules are dropped
/// when the board loads: nobody could tell which of them were real.
const RULES: u32 = 2;
/// The notes behind the sell offers are looked up on the chain at most this often.
const BACKING_CHECK_SECS: u64 = 10;
/// The longest proof of funds a board reads (hex): a transfer is about 100 KB.
const MAX_RESERVE_HEX: usize = 2 * krypton_core::params::MAX_TX_BYTES_V1;

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
    message_bytes: usize,
    trades: Vec<(Trade, u64)>,
    /// Krypton coin offers and verified coin swaps (kept apart from the bitcoin board,
    /// which older wallets read).
    coin_offers: HashMap<String, CoinOffer>,
    coin_trades: Vec<(CoinTrade, u64)>,
    /// Claims of locked GNR seen on chain, by hash lock, and the highest block scanned.
    claims: HashMap<String, Claim>,
    scanned: Option<u64>,
    loaded: bool,
    /// The notes proven to be behind each open sell offer.
    backing: HashMap<String, Backing>,
    backing_checked: u64,
    /// Swaps in progress whose bitcoin has been paid, as their makers report it.
    funded: HashSet<String>,
}

impl Inner {
    /// Whether coins of `b` already back an open offer or a swap in progress.
    fn backs_another(&self, b: &Backing) -> bool {
        self.backing
            .values()
            .any(|x| x.nullifiers.iter().any(|n| b.nullifiers.contains(n)))
    }

    /// Lists a new offer. A sell offer comes with its checked proof of funds, and its
    /// coins must not be behind anything else on the board.
    fn list(&mut self, o: Offer, backing: Option<Backing>, t: u64) -> Result<(), &'static str> {
        if self.offers.len() >= MAX_OFFERS {
            return Err("the board is full; try again later");
        }
        if self.offers.contains_key(&o.id)
            || self.pending.contains_key(&o.id)
            || self.coin_offers.contains_key(&o.id)
        {
            return Err("an offer with this id exists");
        }
        if o.side == Side::SellGnr {
            let Some(b) = backing else {
                return Err(board::RESERVE_MISSING);
            };
            if self.backs_another(&b) {
                return Err("this GNR already backs another offer on the board");
            }
            self.backing.insert(o.id.clone(), b);
        }
        self.seen.insert(o.id.clone(), t);
        self.offers.insert(o.id.clone(), o);
        Ok(())
    }

    /// Moves a taken offer to "in progress". Its coins now back that swap: they stay
    /// spoken for until the swap is over, so they cannot be listed a second time.
    fn start(&mut self, id: &str, t: u64) {
        if let Some(o) = self.offers.remove(id) {
            self.seen.remove(id);
            self.pending.insert(id.to_string(), (o, t));
        }
    }

    /// Takes a bitcoin offer, or its swap in progress, off the board and frees its coins.
    fn take_down(&mut self, id: &str) {
        self.offers.remove(id);
        self.pending.remove(id);
        self.backing.remove(id);
        if !self.coin_offers.contains_key(id) {
            self.seen.remove(id);
        }
        self.pending_seen.remove(id);
        self.funded.remove(id);
    }
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
        // Offers whose maker has been offline for a week are dropped; their makers'
        // wallets see that in their next ping and free the GNR. Any price may stay.
        let seen = &i.seen;
        i.offers
            .retain(|id, _| seen.get(id).is_some_and(|s| s + STALE_SECS > t));
        i.coin_offers
            .retain(|id, o| o.expires > t && seen.get(id).is_some_and(|s| s + STALE_SECS > t));
        let ps = &i.pending_seen;
        i.pending.retain(|id, (_, at)| {
            *at + PENDING_SECS > t && ps.get(id).copied().unwrap_or(*at) + PENDING_IDLE_SECS > t
        });
        let pids: std::collections::HashSet<String> = i.pending.keys().cloned().collect();
        i.pending_seen.retain(|id, _| pids.contains(id));
        i.funded.retain(|id| pids.contains(id));
        // Coins stay spoken for while their offer is open and while its swap runs.
        let (offers, pending) = (&i.offers, &i.pending);
        i.backing
            .retain(|id, _| offers.contains_key(id) || pending.contains_key(id));
        let ids: std::collections::HashSet<String> = i
            .offers
            .keys()
            .chain(i.coin_offers.keys())
            .cloned()
            .collect();
        i.seen.retain(|id, _| ids.contains(id));
        let (mut n, mut bytes) = (0, 0);
        i.inbox.retain(|_, q| {
            q.retain(|(at, _)| at + MESSAGE_SECS > t);
            n += q.len();
            bytes += q.iter().map(|(_, d)| d.len()).sum::<usize>();
            !q.is_empty()
        });
        i.messages = n;
        i.message_bytes = bytes;
        i.trades.retain(|(_, at)| at + TRADE_KEEP_SECS > t);
        i.coin_trades.retain(|(_, at)| at + TRADE_KEEP_SECS > t);
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
        if let Ok(b) = std::fs::read(p.with_file_name("swap-coin-trades.json"))
            && let Ok(t) = serde_json::from_slice::<Vec<(CoinTrade, u64)>>(&b)
        {
            i.coin_trades = t;
        }
        // Offers and sealed messages survive restarts too (a node update must not drop
        // them, or a swap in progress could lose a code).
        let mut purged = false;
        if let Ok(b) = std::fs::read(p.with_file_name("swap-board.json"))
            && let Ok(s) = serde_json::from_slice::<Saved>(&b)
        {
            // Offers from before sell offers had to prove their GNR are not carried over.
            // Mailboxes are, and swaps in progress as far as the board can stand for them
            // (see below): a running swap must not lose a code.
            purged = s.rules < RULES;
            if !purged {
                i.offers = s.offers.into_iter().map(|o| (o.id.clone(), o)).collect();
                i.backing = s.backing.into_iter().collect();
            }
            i.funded = s.funded.into_iter().collect();
            i.coin_offers = s
                .coin_offers
                .into_iter()
                .map(|o| (o.id.clone(), o))
                .collect();
            // After a restart every maker gets the usual grace to show it is still online.
            let t = now();
            i.seen = i
                .offers
                .keys()
                .chain(i.coin_offers.keys())
                .map(|id| (id.clone(), t))
                .collect();
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
        // A sell offer without its proof cannot be on the board, whatever the file said:
        // not as an open offer, and not as a swap in progress carried over from before
        // sell offers proved their GNR (the board cannot vouch for it, and nothing would
        // keep its coins from being listed again). Such a swap itself goes on: it needs
        // only the mailboxes.
        let backing = &i.backing;
        i.offers
            .retain(|id, o| o.side != Side::SellGnr || backing.contains_key(id));
        i.pending
            .retain(|id, (o, _)| o.side != Side::SellGnr || backing.contains_key(id));
        if purged {
            self.save_board(i);
        }
    }

    fn write(path: &std::path::Path, b: Vec<u8>) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    fn save(&self, i: &Inner) {
        if let Some(p) = self.path.lock().as_ref() {
            if let Ok(b) = serde_json::to_vec(&i.trades) {
                Board::write(p, b);
            }
            if let Ok(b) = serde_json::to_vec(&i.coin_trades) {
                Board::write(&p.with_file_name("swap-coin-trades.json"), b);
            }
        }
    }

    fn save_board(&self, i: &Inner) {
        let Some(p) = self.path.lock().clone() else {
            return;
        };
        let s = Saved {
            offers: i.offers.values().cloned().collect(),
            coin_offers: i.coin_offers.values().cloned().collect(),
            pending: i.pending.values().cloned().collect(),
            inbox: i
                .inbox
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            rules: RULES,
            backing: i
                .backing
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            funded: i.funded.iter().cloned().collect(),
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
    coin_offers: Vec<CoinOffer>,
    #[serde(default)]
    pending: Vec<(Offer, u64)>,
    inbox: Vec<(String, Vec<(u64, String)>)>,
    #[serde(default)]
    rules: u32,
    #[serde(default)]
    backing: Vec<(String, Backing)>,
    #[serde(default)]
    funded: Vec<String>,
}

fn hash32(s: &str) -> Option<Hash32> {
    hex::decode(s).ok()?.try_into().ok()
}

/// Whether the notes behind a sell offer are still there, on this node's chain.
enum Backed {
    Yes,
    /// Spent, or proven against a chain that is not ours: the offer is not real (any more).
    No(&'static str),
    /// The chain could not be read just now: no verdict.
    Unknown,
}

fn backed(node: &Node, b: &Backing) -> Backed {
    match node.chain.view() {
        Ok(v) => backed_in(&v, b),
        Err(_) => Backed::Unknown,
    }
}

/// [`backed`] against one snapshot of the chain.
fn backed_in(v: &krypton_store::View, b: &Backing) -> Backed {
    let (Some(anchor), Some(n0), Some(n1)) = (
        hash32(&b.anchor),
        hash32(&b.nullifiers[0]),
        hash32(&b.nullifiers[1]),
    ) else {
        return Backed::No("the proof of funds is malformed");
    };
    let known = v.anchor_height(&anchor).is_some();
    let spent = v.nullifier_spent(&n0) || v.nullifier_spent(&n1);
    if v.failed.get() {
        Backed::Unknown
    } else if !known {
        Backed::No("the proof of funds is not for this chain")
    } else if spent {
        Backed::No("the GNR behind this offer has been spent")
    } else {
        Backed::Yes
    }
}

/// Takes down sell offers whose GNR has moved since they were listed. A proof of funds
/// only says the coins were there when it was made; this keeps it true.
fn drop_unbacked(node: &Node, i: &mut Inner, t: u64) {
    if i.backing_checked + BACKING_CHECK_SECS > t {
        return;
    }
    i.backing_checked = t;
    let Ok(v) = node.chain.view() else {
        return;
    };
    // Open offers only: the note behind a swap in progress is spent by that swap's lock.
    let gone: Vec<String> = i
        .backing
        .iter()
        .filter(|(id, _)| i.offers.contains_key(*id))
        .filter(|(_, b)| matches!(backed_in(&v, b), Backed::No(_)))
        .map(|(id, _)| id.clone())
        .collect();
    drop(v);
    if gone.is_empty() {
        return;
    }
    for id in &gone {
        i.take_down(id);
    }
    node.board.save_board(i);
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
    drop_unbacked(&node, &mut i, t);
    let mut list: Vec<&Offer> = i.offers.values().collect();
    list.sort_by(|a, b| a.price().total_cmp(&b.price()));
    let v: Vec<Value> = list
        .into_iter()
        .map(|o| {
            let mut x = serde_json::to_value(o).unwrap_or_default();
            let seen = i.seen.get(&o.id).copied().unwrap_or(0);
            x["online"] = json!(seen + ONLINE_SECS > t);
            x["seen"] = json!(seen);
            // The seller's GNR was proven to be there, and has not moved since.
            x["backed"] = json!(i.backing.contains_key(&o.id));
            x
        })
        .collect();
    let mut p: Vec<Value> = i
        .pending
        .values()
        .map(|(o, at)| json!({"id": o.id, "side": o.side, "gnr": o.gnr, "sats": o.sats, "since": at, "funded": i.funded.contains(&o.id)}))
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
    /// The bitcoin of this swap has been paid into its contract (wallets from 1.0.40).
    #[serde(default)]
    funded: bool,
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
    i.start(&id, t);
    if r.funded {
        i.funded.insert(id.clone());
    }
    i.pending_seen.insert(id, t);
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}

/// An offer as a wallet posts it: the offer and, for a sell offer, its proof of funds.
#[derive(Deserialize)]
pub struct OfferPost {
    #[serde(flatten)]
    offer: Offer,
    /// Hex of a transfer that can never be mined (`krypton_swap::board::check_reserve`).
    #[serde(default)]
    reserve: Option<String>,
}

pub async fn post_offer(State(node): S, Json(p): Json<OfferPost>) -> Response {
    let o = p.offer;
    if let Err(e) = o.validate(node.chain.params.network.as_str(), now()) {
        return bad(e);
    }
    // What needs no work is refused first.
    {
        let mut i = node.board.inner.lock();
        node.board.load(&mut i);
        Board::expire(&mut i, now());
        if i.offers.len() >= MAX_OFFERS {
            return bad("the board is full; try again later");
        }
        if i.offers.contains_key(&o.id)
            || i.pending.contains_key(&o.id)
            || i.coin_offers.contains_key(&o.id)
        {
            return bad("an offer with this id exists");
        }
    }
    // A sell offer is listed only with a proof that its GNR is there. The work is done
    // after the offer's own proof of work was checked and off the async runtime.
    let backing = if o.side == Side::SellGnr {
        let Some(hex_tx) = p.reserve else {
            return bad(board::RESERVE_MISSING);
        };
        if hex_tx.len() > MAX_RESERVE_HEX {
            return bad("the proof of funds is too large");
        }
        let (n, o2) = (node.clone(), o.clone());
        let checked = tokio::task::spawn_blocking(move || -> Result<Backing, String> {
            // A proof of funds is a transfer that expired at block `RESERVE_EXPIRY`; the
            // chain must be past that block for it to be unminable.
            if n.chain.tip().height() < board::RESERVE_EXPIRY {
                return Err("this chain is too young for proofs of funds".into());
            }
            let bytes = hex::decode(&hex_tx).map_err(|_| "bad proof of funds encoding")?;
            let tx =
                Transaction::from_bytes(&bytes).map_err(|e| format!("bad proof of funds: {e}"))?;
            let b = board::check_reserve(&o2, &tx).map_err(|e| e.to_string())?;
            match backed(&n, &b) {
                Backed::Yes => {}
                Backed::No(why) => return Err(why.into()),
                Backed::Unknown => {
                    return Err("the board cannot read its chain right now; try again".into());
                }
            }
            // Coins on their way out are not there to sell.
            let pool = n.mempool.lock();
            if tx.all_nullifiers().iter().any(|nf| pool.spends(nf)) {
                return Err("the GNR behind this offer is being spent".into());
            }
            Ok(b)
        })
        .await;
        match checked {
            Ok(Ok(b)) => Some(b),
            Ok(Err(e)) => return bad(e),
            Err(e) => return bad(e),
        }
    } else {
        None
    };
    // Checked and listed in one step, under the board's lock: of two offers posted at the
    // same moment with the same coins behind them, only the first is listed.
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    if let Err(e) = i.list(o, backing, t) {
        return bad(e);
    }
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
    drop_unbacked(&node, &mut i, t);
    let mut ok = 0;
    let mut missing = vec![];
    for it in r.items {
        if !i.offers.contains_key(&it.id)
            && !i.pending.contains_key(&it.id)
            && !i.coin_offers.contains_key(&it.id)
        {
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
        if i.offers.get(&it.id).is_some_and(|o| o.cancel == h)
            || i.coin_offers.get(&it.id).is_some_and(|o| o.cancel == h)
        {
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
    let h = board::cancel_hash(&secret);
    // Only the entry this secret belongs to is removed: a bitcoin offer (or its swap in
    // progress), else a coin offer.
    let bitcoin = i
        .offers
        .get(&r.id)
        .or_else(|| i.pending.get(&r.id).map(|x| &x.0))
        .map(|o| o.cancel == h);
    let coin = i.coin_offers.get(&r.id).map(|o| o.cancel == h);
    match (bitcoin, coin) {
        (Some(true), _) => i.take_down(&r.id),
        (_, Some(true)) => {
            i.coin_offers.remove(&r.id);
            if bitcoin.is_none() {
                i.seen.remove(&r.id);
            }
        }
        (None, None) => return Json(json!({"ok": true, "gone": true})).into_response(),
        _ => return bad("wrong cancel secret"),
    }
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}

pub async fn post_message(State(node): S, Json(e): Json<Envelope>) -> Response {
    if let Err(x) = e.validate() {
        return bad(x);
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    if i.messages >= MAX_MESSAGES || i.message_bytes + e.data.len() > MAX_MESSAGE_BYTES_TOTAL {
        return bad("the mailbox service is full; try again later");
    }
    let q = i.inbox.entry(e.to.clone()).or_default();
    if q.len() >= MAX_PER_INBOX {
        return bad("this inbox is full");
    }
    if q.iter().any(|(_, d)| *d == e.data) {
        return Json(json!({"ok": true})).into_response();
    }
    let len = e.data.len();
    q.push_back((t, e.data));
    i.messages += 1;
    i.message_bytes += len;
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
    // "In progress" counts swaps whose bitcoin has been paid. An offer that was only
    // taken proves nothing: taking costs nothing, and the taker may never pay.
    let pending: Vec<(Offer, u64)> = i
        .pending
        .values()
        .filter(|(o, _)| i.funded.contains(&o.id))
        .cloned()
        .collect();
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
    let taken = i.pending.len() - i.pending.keys().filter(|id| i.funded.contains(*id)).count();
    Json(json!({"market": m, "recent": recent, "window_secs": board::PRICE_WINDOW_SECS, "taken_unpaid": taken}))
        .into_response()
}

pub async fn coin_offers(State(node): S) -> Response {
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    let mut list: Vec<&CoinOffer> = i.coin_offers.values().collect();
    list.sort_by(|a, b| a.coin.cmp(&b.coin).then(a.price().total_cmp(&b.price())));
    let offers: Vec<Value> = list
        .into_iter()
        .map(|o| {
            let mut x = serde_json::to_value(o).unwrap_or_default();
            let seen = i.seen.get(&o.id).copied().unwrap_or(0);
            x["online"] = json!(seen + ONLINE_SECS > t);
            x["seen"] = json!(seen);
            x
        })
        .collect();
    let mut by_coin: HashMap<&str, Vec<(CoinTrade, u64)>> = HashMap::new();
    for (x, at) in &i.coin_trades {
        by_coin
            .entry(x.coin.as_str())
            .or_default()
            .push((x.clone(), *at));
    }
    let markets: serde_json::Map<String, Value> = by_coin
        .iter()
        .filter_map(|(c, v)| {
            board::coin_market(v, t)
                .map(|(price, swaps)| (c.to_string(), json!({"price": price, "swaps": swaps})))
        })
        .collect();
    let recent: Vec<Value> = i
        .coin_trades
        .iter()
        .rev()
        .take(100)
        .map(|(x, at)| json!({"at": at, "coin": x.coin, "gnr": x.gnr, "units": x.units, "price": x.price(), "txid": x.txid}))
        .collect();
    Json(json!({"offers": offers, "markets": markets, "recent": recent, "now": t})).into_response()
}

pub async fn post_coin_offer(State(node): S, Json(o): Json<CoinOffer>) -> Response {
    if let Err(e) = o.validate(node.chain.params.network.as_str(), now()) {
        return bad(e);
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    Board::expire(&mut i, now());
    if i.offers.len() + i.coin_offers.len() >= MAX_OFFERS {
        return bad("the board is full; try again later");
    }
    if i.coin_offers.contains_key(&o.id)
        || i.offers.contains_key(&o.id)
        || i.pending.contains_key(&o.id)
    {
        return bad("an offer with this id exists");
    }
    i.seen.insert(o.id.clone(), now());
    i.coin_offers.insert(o.id.clone(), o);
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}

/// A completed coin swap: checked against the mined transaction before it counts.
pub async fn post_coin_trade(State(node): S, Json(x): Json<CoinTrade>) -> Response {
    if let Err(e) = x.validate(node.chain.params.network.as_str()) {
        return bad(e);
    }
    let n = node.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        {
            let mut i = n.board.inner.lock();
            n.board.load(&mut i);
            if i.coin_trades.iter().any(|(y, _)| y.txid == x.txid) {
                return Ok(false);
            }
        }
        let txid: [u8; 32] = hex::decode(&x.txid)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or("bad txid")?;
        let h = n
            .chain
            .tx_height(&txid)
            .map_err(|e| e.to_string())?
            .ok_or("the swap is not mined yet")?;
        let b = n
            .chain
            .block_at(h)
            .map_err(|e| e.to_string())?
            .ok_or("block missing")?;
        let tx = b
            .txs
            .iter()
            .find(|t| t.txid() == txid)
            .ok_or("transaction missing")?;
        x.verify(tx).map_err(|e| e.to_string())?;
        let mut i = n.board.inner.lock();
        if i.coin_trades.iter().any(|(y, _)| y.txid == x.txid) {
            return Ok(false);
        }
        if i.coin_trades.len() >= MAX_TRADES {
            i.coin_trades.remove(0);
        }
        i.coin_trades.push((x, b.header.timestamp));
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

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_800_000_000;

    fn offer(id: &str, side: Side) -> Offer {
        Offer {
            id: id.into(),
            network: "devnet".into(),
            btc_network: "regtest".into(),
            side,
            gnr: 5 * 100_000_000,
            sats: 50_000,
            key: String::new(),
            expires: T + 3600,
            cancel: String::new(),
            nonce: 0,
        }
    }

    fn backing(a: &str, b: &str) -> Option<Backing> {
        Some(Backing {
            nullifiers: [a.into(), b.into()],
            anchor: "root".into(),
        })
    }

    #[test]
    fn a_sell_offer_is_listed_only_with_its_proof() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), None, T),
            Err(board::RESERVE_MISSING)
        );
        assert!(i.offers.is_empty() && i.backing.is_empty());
        // Buying promises bitcoin, which no GNR board can check: no proof is asked for.
        assert_eq!(i.list(offer("b", Side::BuyGnr), None, T), Ok(()));
        assert!(i.offers.contains_key("b") && i.backing.is_empty());
    }

    #[test]
    fn the_same_coins_back_one_thing_at_a_time() {
        let twice = Err("this GNR already backs another offer on the board");
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), backing("n1", "d1"), T),
            Ok(())
        );
        // The same note behind a second offer, as either of the proof's two inputs.
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n1", "d2"), T),
            twice
        );
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("d2", "n1"), T),
            twice
        );
        assert!(!i.offers.contains_key("b") && !i.backing.contains_key("b"));
        // The offer is taken. Its coins back the swap now, and until that is over they
        // cannot be listed again (the maker could otherwise move an offer to "in
        // progress" by itself and list the same coins once more).
        i.start("a", T);
        assert!(i.offers.is_empty() && i.pending.contains_key("a"));
        Board::expire(&mut i, T + 60);
        assert!(i.backing.contains_key("a"));
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n1", "d2"), T),
            twice
        );
        // The same id cannot be used again either while the swap runs.
        assert!(
            i.list(offer("a", Side::SellGnr), backing("n9", "d9"), T)
                .is_err()
        );
        // The swap is over (its maker takes the entry down): the coins are free again.
        i.take_down("a");
        assert!(i.backing.is_empty() && i.pending.is_empty());
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n1", "d2"), T),
            Ok(())
        );
        // Other coins are another matter.
        assert_eq!(
            i.list(offer("c", Side::SellGnr), backing("n2", "d3"), T),
            Ok(())
        );
        assert_eq!(i.backing.len(), 2);
    }

    #[test]
    fn coins_are_free_again_when_their_entry_runs_out() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), backing("n1", "d1"), T),
            Ok(())
        );
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n2", "d2"), T),
            Ok(())
        );
        i.start("b", T);
        // The open offer ends; the swap in progress is still confirmed by its maker.
        i.pending_seen.insert("b".into(), T + 3600);
        Board::expire(&mut i, T + 3601);
        assert!(!i.backing.contains_key("a") && i.backing.contains_key("b"));
        // A swap in progress nobody confirms any more is dropped, and frees its coins.
        Board::expire(&mut i, T + 3600 + PENDING_IDLE_SECS + 1);
        assert!(i.pending.is_empty() && i.backing.is_empty());
        assert_eq!(
            i.list(offer("c", Side::SellGnr), backing("n2", "d9"), T),
            Ok(())
        );
    }

    /// A board file from before sell offers proved their GNR. None of its offers is
    /// carried over, nor a sell swap in progress the board cannot stand for; mailboxes
    /// and the other swaps in progress are. This happens once: what is listed under the
    /// new rules survives the next start.
    #[test]
    fn a_board_from_before_the_rules_starts_without_its_offers() {
        let dir = std::env::temp_dir().join(format!("gk-board-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = now();
        let at = |id: &str, side: Side| {
            let mut o = offer(id, side);
            o.expires = t + 3600;
            o
        };
        // What 1.0.39 wrote: no rules, no backing.
        let old = json!({
            "offers": [at("a", Side::SellGnr), at("b", Side::BuyGnr)],
            "coin_offers": [],
            "pending": [[at("c", Side::SellGnr), t], [at("d", Side::BuyGnr), t]],
            "inbox": [["ff".repeat(32), [[t, "sealed"]]]],
        });
        let file = dir.join("swap-board.json");
        std::fs::write(&file, serde_json::to_vec(&old).unwrap()).unwrap();
        let open = || {
            let b = Board::default();
            b.set_path(dir.join("swap-trades.json"));
            let mut i = Inner::default();
            b.load(&mut i);
            (b, i)
        };
        let (b, mut i) = open();
        assert!(i.offers.is_empty() && i.backing.is_empty());
        assert_eq!(i.pending.keys().collect::<Vec<_>>(), ["d"]);
        assert_eq!(i.messages, 1);
        let saved: Saved = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!((saved.rules, saved.offers.len()), (RULES, 0));
        // Listed under the new rules, taken, and the node starts again.
        assert_eq!(
            i.list(at("e", Side::SellGnr), backing("n1", "d1"), t),
            Ok(())
        );
        assert_eq!(
            i.list(at("f", Side::SellGnr), backing("n2", "d2"), t),
            Ok(())
        );
        i.start("f", t);
        b.save_board(&i);
        let (_, mut i) = open();
        assert!(i.offers.contains_key("e") && i.pending.contains_key("f"));
        assert!(i.pending.contains_key("d"));
        assert_eq!(i.backing.len(), 2);
        // The coins of the swap in progress are still spoken for after the restart.
        assert!(
            i.list(at("g", Side::SellGnr), backing("n2", "d3"), t)
                .is_err()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn taking_an_offer_down_leaves_others_alone() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), backing("n1", "d1"), T),
            Ok(())
        );
        assert_eq!(i.list(offer("b", Side::BuyGnr), None, T), Ok(()));
        i.take_down("a");
        assert!(i.offers.contains_key("b") && i.seen.contains_key("b"));
        assert!(!i.offers.contains_key("a") && !i.seen.contains_key("a"));
    }
}
