//! The swap offer board on a node (`/v1/swap/...`): public offers, sealed mailboxes and
//! the market price from completed swaps (`krypton_swap::board`). Everything lives in
//! memory with hard caps and expiry, except completed swaps, which are kept for good (the
//! trade history), and the board itself, which is written to disk in the background so
//! it survives restarts. The node never sees keys, codes or coins.
//!
//! The board's lock is never held on the async runtime: handlers work on it inside
//! `block_in_place`, and the large answers (offers, market, history) are served from a
//! snapshot rebuilt only when the board changed or the snapshot is a few seconds old.

use crate::guard::ClientKey;
use crate::node::Node;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{StatusCode, header};
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
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAX_OFFERS: usize = 5_000;
const MAX_PER_INBOX: usize = 100;
const MAX_MESSAGES: usize = 5_000;
/// All stored messages together (coin swaps send ~200 KB ones).
const MAX_MESSAGE_BYTES_TOTAL: usize = 96 * 1024 * 1024;
const MESSAGE_SECS: u64 = 24 * 3600;
/// Completed swaps are kept for good (the trade history); past this many the oldest go.
const MAX_TRADES: usize = 200_000;
/// What one client (an address, IPv6 by /48) may put on the board: open offers, and
/// sealed messages in a day, in all and into one inbox.
const MAX_OFFERS_EACH: usize = 200;
const MAX_MESSAGES_A_DAY_EACH: u32 = 2_000;
const MAX_MESSAGES_A_DAY_EACH_INBOX: u32 = 40;
/// How long the snapshots of the board's large answers may be served unchanged.
const OFFERS_TTL: Duration = Duration::from_secs(2);
const MARKET_TTL: Duration = Duration::from_secs(3);
const HISTORY_TTL: Duration = Duration::from_secs(10);
/// The most completed swaps one history page holds.
const HISTORY_PAGE_MAX: usize = 200;
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
    /// Buy offers whose bitcoin was found at the deposit address their maker proved to
    /// own (`board::check_funding`), with that address. It is never published.
    deposits: HashMap<String, String>,
    deposits_checked: u64,
    /// Sell offers whose makers lock GNR only for buyers whose bitcoin is deposited.
    prepaid: HashSet<String>,
    /// Completed swaps whose bitcoin this board found on the Bitcoin chain (1.0.44): by
    /// hash lock, the sat taken out of the swap's contract with its secret, and the key
    /// of the contract that proved the report. Kept on disk with the trades.
    paid: HashMap<String, (u64, String)>,
    /// Swaps whose two wallets reported different amounts: they do not count.
    disputed: HashSet<String>,
    /// When payments were last looked up on the Bitcoin chain, to keep the lookups few:
    /// all of them, and per hash lock (start of its hour, lookups in it).
    lookups: VecDeque<u64>,
    lookups_of: HashMap<String, (u64, u32)>,
    /// Quick Swap offers (1.0.46): their makers' wallets swap parts of them by themselves.
    auto: HashMap<String, board::Auto>,
    /// Swaps in progress that are a part of their offer: the part's GNR and bitcoin.
    part: HashMap<String, (u64, u64)>,
    /// What buyers' deposit addresses are promised to (`board::Promise`): by address,
    /// each promise's request id, bitcoin, release hash and when it was made.
    promises: HashMap<String, Vec<Promised>>,
    /// Who listed each open offer (not kept on disk: after a restart the counts start over).
    lister: HashMap<String, IpAddr>,
    /// Sealed messages each client sent in the current day, in all and into each inbox.
    sent: HashMap<IpAddr, (u64, u32)>,
    sent_to: HashMap<(IpAddr, String), (u64, u32)>,
}

/// Kept on disk with the board: a restart must not let one deposit be promised again.
#[derive(Clone, serde::Serialize, Deserialize)]
struct Promised {
    id: String,
    offer: String,
    sats: u64,
    release: String,
    at: u64,
}

/// At most this many promises in all, on one deposit address, and for one offer (one
/// swap at a time runs per offer; a few more cover swaps that ended unpaid lately).
const MAX_PROMISES: usize = 25_000;
const MAX_PROMISES_EACH: usize = 64;
const MAX_PROMISES_AN_OFFER: usize = 4;

/// Lookups of swap payments on the Bitcoin chain: this many a minute in all, and this
/// many an hour for one swap. Each needs a proof signed by a key of the contract it
/// names and a claim on the GNR chain, but anyone can make a key.
const LOOKUPS_A_MINUTE: usize = 10;
const LOOKUPS_AN_HOUR_EACH: u32 = 12;

/// What is kept on disk of the swaps whose bitcoin was found.
#[derive(serde::Serialize, Deserialize, Default)]
struct SavedPaid {
    paid: Vec<(String, u64, String)>,
    disputed: Vec<String>,
    /// The GNR claims of the completed swaps (1.0.53), so their proofs stay in the history.
    #[serde(default)]
    claims: Vec<(String, Claim)>,
}

impl Inner {
    /// Whether one more look at the Bitcoin chain for the swap `hash` fits the budget;
    /// counts it if so.
    fn lookup_allowed(&mut self, hash: &str, t: u64) -> bool {
        while self.lookups.front().is_some_and(|at| at + 60 <= t) {
            self.lookups.pop_front();
        }
        if self.lookups.len() >= LOOKUPS_A_MINUTE {
            return false;
        }
        if self.lookups_of.len() > 20_000 {
            self.lookups_of.retain(|_, (since, _)| *since + 3600 > t);
        }
        let e = self.lookups_of.entry(hash.to_string()).or_insert((t, 0));
        if e.0 + 3600 <= t {
            *e = (t, 0);
        }
        if e.1 >= LOOKUPS_AN_HOUR_EACH {
            return false;
        }
        e.1 += 1;
        self.lookups.push_back(t);
        true
    }

    /// The swaps the price may rest on by their bitcoin: found on the Bitcoin chain and
    /// not disputed by the swap's other wallet.
    fn verified(&self) -> HashMap<String, u64> {
        self.paid
            .iter()
            .filter(|(h, _)| !self.disputed.contains(*h))
            .map(|(h, (sats, _))| (h.clone(), *sats))
            .collect()
    }

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
        self.lister.remove(id);
        self.offers.remove(id);
        self.pending.remove(id);
        self.backing.remove(id);
        self.deposits.remove(id);
        self.prepaid.remove(id);
        self.auto.remove(id);
        self.part.remove(id);
        if !self.coin_offers.contains_key(id) {
            self.seen.remove(id);
        }
        self.pending_seen.remove(id);
        self.funded.remove(id);
    }

    /// Marks the buy offer `id` (just listed) as having its bitcoin at `address`. One
    /// deposit stands behind one offer.
    fn deposited(&mut self, id: &str, address: String) -> bool {
        if self.deposits.values().any(|a| *a == address) {
            return false;
        }
        self.deposits.insert(id.to_string(), address);
        true
    }
}

#[derive(Default)]
pub struct Board {
    inner: Mutex<Inner>,
    path: Mutex<Option<PathBuf>>,
    /// The board or the completed swaps changed since they were last written (`flush`).
    dirty_board: AtomicBool,
    dirty_trades: AtomicBool,
    /// One write at a time.
    flushing: Mutex<()>,
    /// Bumped by every change the large answers show; their snapshots carry it.
    generation: AtomicU64,
    offers_snap: Mutex<Option<Snap>>,
    market_snap: Mutex<Option<Snap>>,
    summary_snap: Mutex<Option<Snap>>,
}

/// A large answer as it was served, with the board's generation then.
struct Snap {
    generation: u64,
    at: Instant,
    body: Bytes,
}

/// Board work, off the async runtime: the board's lock may be held for a while (a large
/// board, a slow disk), and no runtime thread may wait for it.
fn off_runtime<R>(f: impl FnOnce() -> R) -> R {
    tokio::task::block_in_place(f)
}

fn json_bytes(body: Bytes) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// The client a request came from, as the request guard tells them apart; None for this
/// machine's own software, which no quota applies to.
fn client_of(c: Option<Extension<ClientKey>>) -> Option<IpAddr> {
    c.map(|Extension(k)| k.quota)
}

/// A day's counter: (start of the day, count), started over each day.
fn count_today(slot: &mut (u64, u32), t: u64) -> u32 {
    let day = t - t % 86_400;
    if slot.0 != day {
        *slot = (day, 0);
    }
    slot.1
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
        i.deposits
            .retain(|id, _| offers.contains_key(id) || pending.contains_key(id));
        i.prepaid.retain(|id| offers.contains_key(id));
        // Kept while a part of the offer runs: the part is checked against it.
        i.auto
            .retain(|id, _| offers.contains_key(id) || pending.contains_key(id));
        i.part.retain(|id, _| pending.contains_key(id));
        i.promises.retain(|_, v| {
            v.retain(|p| p.at + board::PROMISE_SECS > t);
            !v.is_empty()
        });
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
        let listed = &i.offers;
        i.lister.retain(|id, _| listed.contains_key(id));
        let day = t - t % 86_400;
        i.sent.retain(|_, (d, _)| *d == day);
        i.sent_to.retain(|_, (d, _)| *d == day);
        // Completed swaps stay (the trade history); only beyond the cap do the oldest go.
        if i.trades.len() > MAX_TRADES {
            let extra = i.trades.len() - MAX_TRADES;
            i.trades.drain(..extra);
        }
        if i.coin_trades.len() > MAX_TRADES {
            let extra = i.coin_trades.len() - MAX_TRADES;
            i.coin_trades.drain(..extra);
        }
        if i.paid.len() + i.disputed.len() > i.trades.len() {
            let kept: HashSet<&str> = i.trades.iter().map(|(x, _)| x.hash.as_str()).collect();
            i.paid.retain(|h, _| kept.contains(h.as_str()));
            i.disputed.retain(|h| kept.contains(h.as_str()));
        }
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
        if let Ok(b) = std::fs::read(p.with_file_name("swap-trades-paid.json"))
            && let Ok(v) = serde_json::from_slice::<SavedPaid>(&b)
        {
            i.paid = v.paid.into_iter().map(|(h, s, k)| (h, (s, k))).collect();
            i.disputed = v.disputed.into_iter().collect();
            i.claims = v.claims.into_iter().collect();
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
                i.deposits = s.deposits.into_iter().collect();
                i.prepaid = s.prepaid.into_iter().collect();
                i.auto = s.auto.into_iter().collect();
            }
            i.part = s
                .part
                .into_iter()
                .map(|(id, g, sats)| (id, (g, sats)))
                .collect();
            // What deposits were promised to stays counted (old ones expire below).
            i.promises = s.promises.into_iter().collect();
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

    /// Something the board shows changed: snapshots are rebuilt on the next request.
    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The completed swaps changed: written by the next `flush`.
    fn save(&self, _i: &Inner) {
        self.dirty_trades.store(true, Ordering::Release);
        self.changed();
    }

    /// The board changed: written by the next `flush` (every couple of seconds, and when
    /// the node stops), never on the request that changed it.
    fn save_board(&self, _i: &Inner) {
        self.dirty_board.store(true, Ordering::Release);
        self.changed();
    }

    /// Writes what changed since the last time. The board's lock is held only to copy
    /// it; turning it into JSON and writing it happen after.
    pub fn flush(&self) {
        let _one = self.flushing.lock();
        let Some(p) = self.path.lock().clone() else {
            return;
        };
        // What is written is always what was read plus what changed, never a board that
        // was not read yet.
        if self.dirty_trades.swap(false, Ordering::AcqRel) {
            let (trades, coin_trades, paid) = {
                let mut i = self.inner.lock();
                self.load(&mut i);
                (
                    i.trades.clone(),
                    i.coin_trades.clone(),
                    Board::saved_paid(&i),
                )
            };
            Board::write_trades(&p, &trades, &coin_trades, &paid);
        }
        if self.dirty_board.swap(false, Ordering::AcqRel) {
            let s = {
                let mut i = self.inner.lock();
                self.load(&mut i);
                Board::saved(&i)
            };
            Board::write_saved(&p, &s);
        }
    }

    fn saved_paid(i: &Inner) -> SavedPaid {
        SavedPaid {
            paid: i
                .paid
                .iter()
                .map(|(h, (s, k))| (h.clone(), *s, k.clone()))
                .collect(),
            disputed: i.disputed.iter().cloned().collect(),
            claims: i
                .trades
                .iter()
                .filter_map(|(x, _)| i.claims.get(&x.hash).map(|c| (x.hash.clone(), c.clone())))
                .collect(),
        }
    }

    fn write_trades(
        p: &std::path::Path,
        trades: &[(Trade, u64)],
        coin_trades: &[(CoinTrade, u64)],
        paid: &SavedPaid,
    ) {
        if let Ok(b) = serde_json::to_vec(trades) {
            Board::write(p, b);
        }
        if let Ok(b) = serde_json::to_vec(coin_trades) {
            Board::write(&p.with_file_name("swap-coin-trades.json"), b);
        }
        if let Ok(b) = serde_json::to_vec(paid) {
            Board::write(&p.with_file_name("swap-trades-paid.json"), b);
        }
    }

    fn write_saved(p: &std::path::Path, s: &Saved) {
        if let Ok(b) = serde_json::to_vec(s) {
            Board::write(&p.with_file_name("swap-board.json"), b);
        }
    }

    /// Writes the board as `i` holds it, now (tests, and a board just loaded).
    #[cfg(test)]
    fn write_board_now(&self, i: &Inner) {
        if let Some(p) = self.path.lock().clone() {
            Board::write_saved(&p, &Board::saved(i));
        }
    }

    /// Serves a large answer from its snapshot while the board has not changed and the
    /// snapshot is younger than `ttl`; otherwise builds it once (others wait for it).
    fn snapshot(
        &self,
        slot: &Mutex<Option<Snap>>,
        ttl: Duration,
        build: impl FnOnce() -> Vec<u8>,
    ) -> Bytes {
        let mut s = slot.lock();
        let g = self.generation.load(Ordering::Acquire);
        if let Some(x) = s.as_ref()
            && x.generation == g
            && x.at.elapsed() < ttl
        {
            return x.body.clone();
        }
        let body = Bytes::from(build());
        *s = Some(Snap {
            generation: g,
            at: Instant::now(),
            body: body.clone(),
        });
        body
    }

    fn saved(i: &Inner) -> Saved {
        Saved {
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
            deposits: i
                .deposits
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            prepaid: i.prepaid.iter().cloned().collect(),
            auto: i.auto.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            part: i
                .part
                .iter()
                .map(|(k, (g, s))| (k.clone(), *g, *s))
                .collect(),
            promises: i
                .promises
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
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
    #[serde(default)]
    deposits: Vec<(String, String)>,
    #[serde(default)]
    prepaid: Vec<String>,
    #[serde(default)]
    auto: Vec<(String, board::Auto)>,
    #[serde(default)]
    part: Vec<(String, u64, u64)>,
    #[serde(default)]
    promises: Vec<(String, Vec<Promised>)>,
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
    let known: HashSet<String> = i.trades.iter().map(|(x, _)| x.hash.clone()).collect();
    let of_a_trade = found
        .iter()
        .any(|(h, _)| known.contains(h) && !i.claims.contains_key(h));
    i.claims.extend(found);
    if i.scanned.is_none_or(|s| s < tip) {
        i.scanned = Some(tip);
    }
    if of_a_trade {
        node.board.save(&i);
    }
}

/// What the GNR chain shows of a completed swap (its amounts are private).
#[derive(Clone, serde::Serialize, Deserialize)]
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
    off_runtime(|| {
        json_bytes(
            node.board
                .snapshot(&node.board.offers_snap, OFFERS_TTL, || offers_body(&node)),
        )
    })
}

fn offers_body(node: &Arc<Node>) -> Vec<u8> {
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    Board::expire(&mut i, now());
    let t = now();
    drop_unbacked(node, &mut i, t);
    drop(i);
    recheck_deposits(node);
    let i = node.board.inner.lock();
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
            // The buyer's bitcoin was found at a deposit address of the buyer's wallet.
            x["deposited"] = json!(i.deposits.contains_key(&o.id));
            // The seller locks GNR only for a buyer whose bitcoin is deposited.
            x["prepaid"] = json!(i.prepaid.contains(&o.id));
            // A Quick Swap offer: its maker's wallet swaps parts of it by itself.
            if let Some(a) = i.auto.get(&o.id) {
                x["auto"] = json!(a);
            }
            x
        })
        .collect();
    let mut p: Vec<Value> = i
        .pending
        .values()
        .map(|(o, at)| {
            let (g, s) = i.part.get(&o.id).copied().unwrap_or((o.gnr, o.sats));
            json!({"id": o.id, "side": o.side, "gnr": g, "sats": s, "since": at, "funded": i.funded.contains(&o.id), "part": i.part.contains_key(&o.id)})
        })
        .collect();
    p.sort_by_key(|x| std::cmp::Reverse(x["since"].as_u64()));
    serde_json::to_vec(&json!({"offers": v, "in_progress": p, "now": now()})).unwrap_or_default()
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
    /// The swap is for a part of a Quick Swap offer (1.0.46): its GNR and bitcoin.
    #[serde(default)]
    part: Option<board::Part>,
}

/// The maker of a taken offer moves it to "in progress" (shown, no longer takeable), and
/// says every few minutes that its swap still runs. Only the offer stored on the board can
/// move, proven with its cancel secret: the request's own copy of the offer is ignored.
pub async fn post_progress(State(node): S, Json(r): Json<ProgressReq>) -> Response {
    off_runtime(move || post_progress_now(&node, r))
}

fn post_progress_now(node: &Arc<Node>, r: ProgressReq) -> Response {
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
    let part = match stored {
        None => return bad("no such offer on the board"),
        Some(o) if o.cancel != board::cancel_hash(&secret) => return bad("wrong cancel secret"),
        // A part is no more than its offer, at no other price than the offer's.
        // Only a Quick Swap offer has parts.
        Some(o) => match &r.part {
            Some(p) => {
                if p.id != id || board::part_ok(o, i.auto.get(&id), p.gnr, p.sats).is_err() {
                    return bad("this part does not fit the offer");
                }
                Some((p.gnr, p.sats))
            }
            None => None,
        },
    };
    i.start(&id, t);
    if let Some(p) = part {
        i.part.insert(id.clone(), p);
    }
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
    /// A buy offer whose bitcoin is deposited: the proof of the deposit address
    /// (`krypton_swap::board::check_funding`).
    #[serde(default)]
    funding: Option<board::Funding>,
    /// A sell offer for buyers whose bitcoin is deposited first (the maker's wallet
    /// checks that itself; the board only shows it).
    #[serde(default)]
    prepaid: bool,
    /// A Quick Swap offer (1.0.46): sold or bought in parts of at least this size.
    #[serde(default)]
    auto: Option<board::Auto>,
}

pub async fn post_offer(
    State(node): S,
    client: Option<Extension<ClientKey>>,
    Json(p): Json<OfferPost>,
) -> Response {
    let client = client_of(client);
    let o = p.offer;
    if let Err(e) = o.validate(node.chain.params.network.as_str(), now()) {
        return bad(e);
    }
    if let Some(a) = &p.auto
        && let Err(e) = board::auto_ok(&o, a)
    {
        return bad(e);
    }
    // What needs no work is refused first.
    let refused = off_runtime(|| {
        let mut i = node.board.inner.lock();
        node.board.load(&mut i);
        Board::expire(&mut i, now());
        if i.offers.len() >= MAX_OFFERS {
            return Some("the board is full; try again later");
        }
        if i.offers.contains_key(&o.id)
            || i.pending.contains_key(&o.id)
            || i.coin_offers.contains_key(&o.id)
        {
            return Some("an offer with this id exists");
        }
        if let Some(c) = client
            && i.lister.values().filter(|x| **x == c).count() >= MAX_OFFERS_EACH
        {
            return Some("too many open offers from your address");
        }
        None
    });
    if let Some(why) = refused {
        return bad(why);
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
    // A buy offer may say that its bitcoin is deposited. The claim is checked on the
    // Bitcoin chain if this board can read it, and the offer is marked only then.
    let deposit = match (&p.funding, o.side) {
        (Some(f), Side::BuyGnr) => {
            let address = match board::check_funding(&o, f) {
                Ok(a) => a,
                Err(e) => return bad(e),
            };
            match btc_api() {
                None => None,
                Some(api) => {
                    let (a, need) = (address.clone(), o.sats);
                    match tokio::task::spawn_blocking(move || deposit_at(&api, &a, need)).await {
                        Ok(Ok(true)) => Some(address.to_string()),
                        Ok(Ok(false)) => {
                            return bad(
                                "the bitcoin of this offer is not at its deposit address, confirmed and unspent",
                            );
                        }
                        // The Bitcoin server did not answer: list the offer unmarked.
                        Ok(Err(_)) | Err(_) => None,
                    }
                }
            }
        }
        _ => None,
    };
    // Checked and listed in one step, under the board's lock: of two offers posted at the
    // same moment with the same coins behind them, only the first is listed.
    off_runtime(move || list_offer(&node, client, o, p.prepaid, p.auto, backing, deposit))
}

fn list_offer(
    node: &Arc<Node>,
    client: Option<IpAddr>,
    o: Offer,
    prepaid: bool,
    auto: Option<board::Auto>,
    backing: Option<Backing>,
    deposit: Option<String>,
) -> Response {
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    if deposit
        .as_ref()
        .is_some_and(|a| i.deposits.values().any(|x| x == a))
    {
        return bad("this bitcoin deposit already stands behind another offer on the board");
    }
    let id = o.id.clone();
    let prepaid = prepaid && o.side == Side::SellGnr;
    if let Err(e) = i.list(o, backing, t) {
        return bad(e);
    }
    if let Some(c) = client {
        i.lister.insert(id.clone(), c);
    }
    if prepaid {
        i.prepaid.insert(id.clone());
    }
    if let Some(a) = auto {
        i.auto.insert(id.clone(), a);
    }
    let marked = deposit.is_some_and(|a| i.deposited(&id, a));
    node.board.save_board(&i);
    Json(json!({"ok": true, "deposited": marked})).into_response()
}

/// The Bitcoin server this board reads deposits of buy offers from (`KRYPTON_BTC_API`:
/// an Esplora URL, or `rpc:` and a Bitcoin Core URL). Without it buy offers are listed
/// as before, unmarked.
fn btc_api() -> Option<String> {
    std::env::var("KRYPTON_BTC_API")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether confirmed, unspent outputs worth at least `need` sat pay `address`.
fn deposit_at(
    api: &str,
    address: &krypton_swap::bitcoin::Address,
    need: u64,
) -> anyhow::Result<bool> {
    let btc = krypton_swap::btc::connect(api)?;
    let have = btc
        .unspent(address)?
        .iter()
        .filter(|u| u.height.is_some())
        .fold(0u64, |a, u| a.saturating_add(u.value.to_sat()));
    Ok(have >= need)
}

/// How often the deposits behind open buy offers are looked up again.
const DEPOSIT_CHECK_SECS: u64 = 300;

/// Looks the deposits of open buy offers up again and takes the mark off those whose
/// bitcoin has moved. The lookups run off the board's lock.
fn recheck_deposits(node: &Arc<Node>) {
    let Some(api) = btc_api() else { return };
    let list: Vec<(String, String, u64)> = {
        let mut i = node.board.inner.lock();
        let t = now();
        if i.deposits_checked + DEPOSIT_CHECK_SECS > t {
            return;
        }
        i.deposits_checked = t;
        i.deposits
            .iter()
            .filter_map(|(id, a)| i.offers.get(id).map(|o| (id.clone(), a.clone(), o.sats)))
            .take(100)
            .collect()
    };
    if list.is_empty() {
        return;
    }
    let node = node.clone();
    tokio::task::spawn_blocking(move || {
        use std::str::FromStr;
        let gone: Vec<String> = list
            .into_iter()
            .filter(|(_, a, need)| {
                krypton_swap::bitcoin::Address::from_str(a)
                    .ok()
                    .map(|a| a.assume_checked())
                    // Only a clear "not there" takes the mark off, not a failed lookup.
                    .is_some_and(|a| matches!(deposit_at(&api, &a, *need), Ok(false)))
            })
            .map(|(id, _, _)| id)
            .collect();
        if gone.is_empty() {
            return;
        }
        let mut i = node.board.inner.lock();
        for id in &gone {
            if i.offers.contains_key(id) {
                i.deposits.remove(id);
            }
        }
        node.board.save_board(&i);
    });
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
    off_runtime(move || ping_now(&node, r))
}

fn ping_now(node: &Arc<Node>, r: PingReq) -> Response {
    if r.items.len() > 500 {
        return bad("too many offers in one ping");
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    drop_unbacked(node, &mut i, t);
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
    off_runtime(move || cancel_offer_now(&node, r))
}

fn cancel_offer_now(node: &Arc<Node>, r: CancelReq) -> Response {
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

pub async fn post_message(
    State(node): S,
    client: Option<Extension<ClientKey>>,
    Json(e): Json<Envelope>,
) -> Response {
    let client = client_of(client);
    off_runtime(move || post_message_now(&node, client, e))
}

fn post_message_now(node: &Arc<Node>, client: Option<IpAddr>, e: Envelope) -> Response {
    if let Err(x) = e.validate() {
        return bad(x);
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    if i.inbox
        .get(&e.to)
        .is_some_and(|q| q.iter().any(|(_, d)| *d == e.data))
    {
        return Json(json!({"ok": true})).into_response();
    }
    // One client cannot fill the service, nor one inbox: a day's quota of messages in
    // all and into each inbox (a swap needs a handful).
    if let Some(c) = client {
        let all = count_today(i.sent.entry(c).or_insert((0, 0)), t);
        let to = count_today(i.sent_to.entry((c, e.to.clone())).or_insert((0, 0)), t);
        if all >= MAX_MESSAGES_A_DAY_EACH || to >= MAX_MESSAGES_A_DAY_EACH_INBOX {
            return bad("too many messages from your address today; try again tomorrow");
        }
    }
    if i.messages >= MAX_MESSAGES || i.message_bytes + e.data.len() > MAX_MESSAGE_BYTES_TOTAL {
        return bad("the mailbox service is full; try again later");
    }
    // A full inbox lets its oldest message go (its wallet reads it every few seconds, so
    // what it needed is read already): a flood cannot shut a mailbox.
    let q = i.inbox.entry(e.to.clone()).or_default();
    let mut freed = 0;
    let mut freed_n = 0;
    while q.len() >= MAX_PER_INBOX {
        if let Some((_, old)) = q.pop_front() {
            freed += old.len();
            freed_n += 1;
        }
    }
    let len = e.data.len();
    q.push_back((t, e.data));
    i.messages = i.messages + 1 - freed_n;
    i.message_bytes = i.message_bytes + len - freed;
    if let Some(c) = client {
        i.sent.entry(c).or_insert((0, 0)).1 += 1;
        i.sent_to.entry((c, e.to)).or_insert((0, 0)).1 += 1;
    }
    node.board.save_board(&i);
    Json(json!({"ok": true})).into_response()
}

pub async fn inbox(State(node): S, Path(id): Path<String>) -> Response {
    off_runtime(move || inbox_now(&node, id))
}

fn inbox_now(node: &Arc<Node>, id: String) -> Response {
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

/// A trade report as wallets post it: the trade (its proof of work covers exactly that),
/// and from 1.0.44 the proof that lets the board find the swap's bitcoin.
#[derive(Deserialize)]
pub struct TradePost {
    #[serde(flatten)]
    trade: Trade,
    #[serde(default)]
    paid: Option<board::Paid>,
}

/// What the Bitcoin chain shows was taken out of the contract `p` names with `secret`.
fn paid_on_chain(api: &str, t: &Trade, p: &board::Paid, secret: &str) -> anyhow::Result<u64> {
    let (htlc, funding) = board::check_paid(t, p)?;
    let secret: [u8; 32] = hash32(secret).ok_or_else(|| anyhow::anyhow!("no secret"))?;
    let btc = krypton_swap::btc::connect(api)?;
    krypton_swap::btc::claimed_from(btc.as_ref(), &htlc, &funding, p.funded_height, &secret)
}

pub async fn post_trade(State(node): S, Json(p): Json<TradePost>) -> Response {
    let TradePost { trade: t, paid } = p;
    if let Err(e) = t.validate(node.chain.params.network.as_str()) {
        return bad(e);
    }
    let n = node.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        scan_claims(&n);
        // The swap's secret, as its claim on the GNR chain shows it, and who has proven
        // this swap already.
        let (secret, proven_by) = {
            let mut i = n.board.inner.lock();
            n.board.load(&mut i);
            let Some(c) = i.claims.get(&t.hash) else {
                return Err("no claim with this hash lock on chain yet".into());
            };
            (
                c.secret.clone(),
                i.paid.get(&t.hash).map(|(_, key)| key.clone()),
            )
        };
        // A report with a proof: the board looks on the Bitcoin chain for itself, off
        // its lock. One look per report, and none for a wallet that has proven already.
        let mut found: Option<(u64, String)> = None;
        if let (Some(p), Some(api)) = (paid.as_ref(), btc_api()) {
            board::check_paid(&t, p).map_err(|e| e.to_string())?;
            let key = p.key.to_ascii_lowercase();
            if proven_by.as_deref() != Some(key.as_str()) {
                if !n.board.inner.lock().lookup_allowed(&t.hash, now()) {
                    return Err(
                        "the board is busy checking swap payments; the report is sent again by itself"
                            .into(),
                    );
                }
                let sats = paid_on_chain(&api, &t, p, &secret)
                    .map_err(|e| format!("the bitcoin of this swap could not be looked up: {e}"))?;
                // The payout is the payment less its fee: at least half of what is reported.
                if sats == 0 || sats.saturating_mul(2) < t.sats {
                    return Err(
                        "the bitcoin of this swap was not found on the Bitcoin chain yet; the report is sent again by itself"
                            .into(),
                    );
                }
                found = Some((sats, key));
            }
        }
        let mut i = n.board.inner.lock();
        let at = i.trades.iter().position(|(x, _)| x.hash == t.hash);
        let mut added = false;
        match (found, at) {
            (Some(f), Some(k)) => {
                let same = i.trades[k].0.gnr == t.gnr && i.trades[k].0.sats == t.sats;
                if i.paid.contains_key(&t.hash) {
                    // The swap's other wallet says something else: neither counts.
                    if !same {
                        i.disputed.insert(t.hash.clone());
                    }
                } else {
                    // The first proven report stands in for an unproven one.
                    if !same {
                        i.trades[k].0 = t.clone();
                    }
                    i.paid.insert(t.hash.clone(), f);
                }
            }
            (Some(f), None) => {
                i.paid.insert(t.hash.clone(), f);
                added = true;
            }
            (None, None) => added = true,
            (None, Some(_)) => {}
        }
        if added {
            if i.trades.len() >= MAX_TRADES {
                i.trades.remove(0);
            }
            i.trades.push((t.clone(), now()));
        }
        n.board.save(&i);
        let verified = i.paid.contains_key(&t.hash) && !i.disputed.contains(&t.hash);
        Ok(json!({"ok": true, "added": added, "verified": verified}))
    })
    .await;
    match r {
        Ok(Ok(v)) => Json(v).into_response(),
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
        .map(|(o, at)| {
            // A part of an offer counts for what it is, not for the whole offer.
            let mut o = o.clone();
            if let Some((g, s)) = i.part.get(&o.id) {
                (o.gnr, o.sats) = (*g, *s);
            }
            (o, *at)
        })
        .collect();
    board::market(&i.trades, &i.verified(), &offers, &pending, t)
}

/// What Quick Swap can do right now: the Quick Swap offers whose makers are online and
/// whose coins are there (sell offers: GNR proven and unmoved; buy offers: bitcoin found
/// at the deposit), on each side.
fn quick(i: &Inner, t: u64) -> Value {
    let live = |o: &&Offer| {
        i.auto.contains_key(&o.id)
            && i.seen.get(&o.id).is_some_and(|s| s + ONLINE_SECS > t)
            && match o.side {
                Side::SellGnr => i.backing.contains_key(&o.id),
                Side::BuyGnr => i.deposits.contains_key(&o.id),
            }
    };
    let side = |s: Side| {
        let v: Vec<&Offer> = i
            .offers
            .values()
            .filter(live)
            .filter(|o| o.side == s)
            .collect();
        let best = match s {
            Side::SellGnr => v.iter().map(|o| o.price()).min_by(f64::total_cmp),
            Side::BuyGnr => v.iter().map(|o| o.price()).max_by(f64::total_cmp),
        };
        let min = v
            .iter()
            .filter_map(|o| i.auto.get(&o.id).map(|a| a.min))
            .min();
        json!({
            "offers": v.len(),
            "gnr": v.iter().map(|o| o.gnr).sum::<u64>(),
            "sats": v.iter().map(|o| o.sats).sum::<u64>(),
            "best": best,
            "min_gnr": min,
        })
    };
    let busy = i
        .pending
        .keys()
        .filter(|id| i.part.contains_key(*id))
        .count();
    json!({
        "sell": side(Side::SellGnr),
        "buy": side(Side::BuyGnr),
        "parts_in_progress": busy,
        "max_parts": board::MAX_PARTS,
    })
}

pub async fn market(State(node): S) -> Response {
    off_runtime(|| {
        json_bytes(
            node.board
                .snapshot(&node.board.market_snap, MARKET_TTL, || {
                    // Each swap with what the GNR chain shows of it: its claim, block, time and secret.
                    scan_claims(&node);
                    market_body(&node)
                }),
        )
    })
}

/// One completed swap as the history and the market show it.
fn trade_json(i: &Inner, x: &Trade, at: u64, tip: u64) -> Value {
    let c = i.claims.get(&x.hash);
    json!({
        "at": at, "gnr": x.gnr, "sats": x.sats, "price": x.price(), "hash": x.hash,
        // Its bitcoin was found on the Bitcoin chain, paid through the swap's contract.
        "verified": i.paid.contains_key(&x.hash) && !i.disputed.contains(&x.hash),
        "claim": c,
        "confirmations": c.map(|c| (tip + 1).saturating_sub(c.height)),
    })
}

fn market_body(node: &Arc<Node>) -> Vec<u8> {
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
        .map(|(x, at)| trade_json(&i, x, *at, tip))
        .collect();
    let taken = i.pending.len() - i.pending.keys().filter(|id| i.funded.contains(*id)).count();
    let q = quick(&i, t);
    serde_json::to_vec(&json!({"market": m, "recent": recent, "window_secs": board::PRICE_WINDOW_SECS, "taken_unpaid": taken, "quick": q, "now": t}))
        .unwrap_or_default()
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    /// How many of the newest swaps to skip (pages of `limit`).
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

/// Every completed swap, newest first, a page at a time (1.0.53): the trade history.
pub async fn history(State(node): S, Query(q): Query<HistoryQuery>) -> Response {
    off_runtime(|| {
        let limit = q.limit.unwrap_or(50).clamp(1, HISTORY_PAGE_MAX);
        let tip = node.chain.tip().height();
        let mut i = node.board.inner.lock();
        node.board.load(&mut i);
        let total = i.trades.len();
        let page: Vec<Value> = i
            .trades
            .iter()
            .rev()
            .skip(q.offset)
            .take(limit)
            .map(|(x, at)| trade_json(&i, x, *at, tip))
            .collect();
        let more = q.offset.saturating_add(page.len()) < total;
        Json(json!({"swaps": page, "offset": q.offset, "more": more, "total": total, "now": now()}))
            .into_response()
    })
}

/// The whole trade history in figures (1.0.53): totals, and day by day the swaps, the GNR
/// and bitcoin they moved and their prices. Swaps whose two wallets disagree do not count.
pub async fn history_summary(State(node): S) -> Response {
    off_runtime(|| {
        json_bytes(
            node.board
                .snapshot(&node.board.summary_snap, HISTORY_TTL, || {
                    summary_body(&node)
                }),
        )
    })
}

fn summary_body(node: &Arc<Node>) -> Vec<u8> {
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let list: Vec<Done> = i
        .trades
        .iter()
        .filter(|(x, _)| !i.disputed.contains(&x.hash) && x.gnr > 0)
        .map(|(x, at)| (*at, x.price(), x.gnr, x.sats, i.paid.contains_key(&x.hash)))
        .collect();
    drop(i);
    serde_json::to_vec(&history_figures(&list, now())).unwrap_or_default()
}

/// A completed swap for the figures: (time, price, GNR, sat, verified).
type Done = (u64, f64, u64, u64, bool);

/// Totals and per-day figures of completed swaps.
fn history_figures(list: &[Done], t: u64) -> Value {
    fn figures(v: &[Done]) -> Value {
        let mut prices: Vec<f64> = v.iter().map(|x| x.1).collect();
        prices.sort_by(f64::total_cmp);
        let gnr: u64 = v.iter().map(|x| x.2).fold(0, u64::saturating_add);
        let sats: u64 = v.iter().map(|x| x.3).fold(0, u64::saturating_add);
        let median = match prices.len() {
            0 => None,
            n if n % 2 == 1 => Some(prices[n / 2]),
            n => Some((prices[n / 2 - 1] + prices[n / 2]) / 2.0),
        };
        json!({
            "swaps": v.len(),
            "verified": v.iter().filter(|x| x.4).count(),
            "gnr": gnr,
            "sats": sats,
            "low": prices.first(),
            "high": prices.last(),
            "median": median,
            // What a GNR cost on average, weighed by the bitcoin moved.
            "vwap": if gnr > 0 { Some(sats as f64 / (gnr as f64 / 1e8)) } else { None },
        })
    }
    let mut days: std::collections::BTreeMap<u64, Vec<Done>> = Default::default();
    for x in list {
        days.entry(x.0 - x.0 % 86_400).or_default().push(*x);
    }
    let mut all = figures(list);
    all["first"] = json!(list.iter().map(|x| x.0).min());
    all["last"] = json!(list.iter().map(|x| x.0).max());
    let last24: Vec<_> = list.iter().filter(|x| x.0 + 86_400 > t).copied().collect();
    json!({
        "all": all,
        "day": figures(&last24),
        "days": days.iter().map(|(d, v)| { let mut f = figures(v); f["day"] = json!(d); f }).collect::<Vec<_>>(),
        "now": t,
    })
}

#[derive(Deserialize)]
pub struct ReleaseReq {
    id: String,
    secret: String,
}

/// A promise as a wallet posts it, with the offer it belongs to.
#[derive(Deserialize)]
pub struct PromisePost {
    #[serde(flatten)]
    promise: board::Promise,
    offer: String,
    /// The offer's cancel secret, when its maker posts the promise.
    #[serde(default)]
    secret: Option<String>,
}

/// A seller's wallet, before it locks GNR for a buyer whose bitcoin waits at a deposit:
/// the buyer's signed word, counted against that deposit. The answer is what the
/// deposit is promised to in all, this promise included; the seller compares that with
/// what the Bitcoin chain shows at the address, and locks only if it is covered. Seen
/// and counted in one step under the board's lock, so two sellers asked at the same
/// moment both see each other.
pub async fn post_promise(State(node): S, Json(r): Json<PromisePost>) -> Response {
    off_runtime(move || post_promise_now(&node, r))
}

fn post_promise_now(node: &Arc<Node>, r: PromisePost) -> Response {
    let p = r.promise;
    let address = match krypton_swap::engine::promise_address(&p) {
        Ok(a) => a.to_string(),
        Err(e) => return bad(e),
    };
    if hex::decode(&p.release).is_err() {
        return bad("bad release hash");
    }
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    // A promise belongs to an offer on this board: the seller's own (proven with the
    // offer's cancel secret), or a buy offer whose deposit it is. So there are never more
    // of them than offers allow, whoever posts them.
    let Some(o) = i
        .offers
        .get(&r.offer)
        .or_else(|| i.pending.get(&r.offer).map(|x| &x.0))
    else {
        return bad("no such offer on the board");
    };
    let maker = r
        .secret
        .as_deref()
        .and_then(|s| hex::decode(s).ok())
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .is_some_and(|s| board::cancel_hash(&s) == o.cancel);
    let its_deposit = o.side == Side::BuyGnr && i.deposits.get(&r.offer) == Some(&address);
    if !maker && !its_deposit {
        return bad("this promise is not for an offer on this board");
    }
    let of_offer = i
        .promises
        .values()
        .flatten()
        .filter(|x| x.offer == r.offer)
        .count();
    let total: usize = i.promises.values().map(Vec::len).sum();
    let v = i.promises.entry(address).or_default();
    if !v.iter().any(|x| x.id == p.id) {
        if v.len() >= MAX_PROMISES_EACH
            || of_offer >= MAX_PROMISES_AN_OFFER
            || total >= MAX_PROMISES
        {
            return bad("too many promises; try again later");
        }
        v.push(Promised {
            id: p.id,
            offer: r.offer,
            sats: p.sats,
            release: p.release,
            at: t,
        });
    }
    let (sum, n) = (
        v.iter().fold(0u64, |a, x| a.saturating_add(x.sats)),
        v.len(),
    );
    node.board.save_board(&i);
    Json(json!({"ok": true, "promised": sum, "promises": n})).into_response()
}

/// Takes a promise back (its swap was paid, or ended), proven with the secret of its
/// release hash.
pub async fn release_promise(State(node): S, Json(r): Json<ReleaseReq>) -> Response {
    off_runtime(move || release_promise_now(&node, r))
}

fn release_promise_now(node: &Arc<Node>, r: ReleaseReq) -> Response {
    let Some(secret) = hex::decode(&r.secret)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    else {
        return bad("bad secret");
    };
    let h = board::release_hash(&secret);
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let mut gone = false;
    i.promises.retain(|_, v| {
        let n = v.len();
        v.retain(|x| !(x.id == r.id && x.release == h));
        gone |= v.len() < n;
        !v.is_empty()
    });
    if gone {
        node.board.save_board(&i);
    }
    Json(json!({"ok": true, "released": gone})).into_response()
}

pub async fn coin_offers(State(node): S) -> Response {
    off_runtime(move || coin_offers_now(&node))
}

fn coin_offers_now(node: &Arc<Node>) -> Response {
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
    off_runtime(move || post_coin_offer_now(&node, o))
}

fn post_coin_offer_now(node: &Arc<Node>, o: CoinOffer) -> Response {
    // Coins exist only once the network has activated them: until then a coin offer could
    // only take up room on the board.
    if node.chain.tip().height() < node.chain.params.assets_height {
        return bad("Krypton coins are not active on this network yet");
    }
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
        b.write_board_now(&i);
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
        b.write_board_now(&i);
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
