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
/// Completed swaps kept in memory (the price and the market read them). The whole history
/// is on disk (`crate::history`); with it, memory keeps the swaps of the last
/// [`MEMORY_TRADE_SECS`] (the price reads 30 days back from any hour of the last 7) and at
/// least the newest [`MEMORY_TRADES_MIN`].
const MAX_TRADES: usize = 200_000;
const MEMORY_TRADE_SECS: u64 = 40 * 24 * 3600;
const MEMORY_TRADES_MIN: usize = 1_000;
/// What one client (an address, IPv6 by /48) may put on the board: open offers, and
/// sealed messages in a day, in all and into one inbox.
const MAX_OFFERS_EACH: usize = 200;
const MAX_MESSAGES_A_DAY_EACH: u32 = 2_000;
const MAX_MESSAGES_A_DAY_EACH_INBOX: u32 = 40;
/// Bytes of messages one client may leave in a day: a few coin swaps' worth, a sixth of
/// the service, so one address cannot fill it.
const MAX_MESSAGE_BYTES_A_DAY_EACH: u64 = 16 * 1024 * 1024;
/// How long the snapshots of the board's large answers may be served unchanged.
const OFFERS_TTL: Duration = Duration::from_secs(2);
const MARKET_TTL: Duration = Duration::from_secs(3);
const HISTORY_TTL: Duration = Duration::from_secs(10);
/// The most completed swaps one history page holds.
const HISTORY_PAGE_MAX: usize = 200;
/// Pages are found by walking from the newest swap: deeper than this, ask with `since`.
const HISTORY_OFFSET_MAX: usize = 2_000_000;
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
/// (`krypton_swap::board::check_reserve`). 3: every buy offer has its bitcoin at a deposit
/// address of the buyer's wallet, found there by this board (`board::check_funding`).
/// Offers stored before rule 2 are dropped when the board loads: nobody could tell which
/// of them were real. From a file of rule 2 the sell offers stay, and the buy offers whose
/// bitcoin this board had found.
const RULES: u32 = 3;
/// The oldest rules whose offers are read at all.
const RULES_READ: u32 = 2;
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
    /// The deposit address behind each buy offer: its maker proved to own it
    /// (`board::check_funding`) and this board found the offer's bitcoin there. It is
    /// never published. Every open buy offer has one.
    deposits: HashMap<String, String>,
    deposits_checked: u64,
    /// Where the next round of looking the deposits up again starts, and how many times
    /// in a row each was clearly not there.
    deposit_cursor: usize,
    deposit_misses: HashMap<String, u8>,
    /// When deposits of new buy offers were looked up on the Bitcoin chain, to keep the
    /// lookups few: all of them, and per client (the minute, lookups in it).
    deposit_lookups: VecDeque<u64>,
    deposit_lookups_of: HashMap<IpAddr, (u64, u32)>,
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
    /// Per client: (day, bytes of messages it left that day).
    sent_bytes: HashMap<IpAddr, (u64, u64)>,
    /// The history on disk is open: memory may let old swaps go.
    hist_ok: bool,
    /// Swaps, and swaps whose status changed, not yet written to the history.
    hist_new: Vec<(Trade, u64)>,
    hist_touch: HashSet<String>,
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

/// Lookups of the deposits of new buy offers: this many a minute in all and from one
/// client. Each comes after the offer's proof of work and a signature by the deposit's
/// key, but anyone can make a key.
const DEPOSIT_LOOKUPS_A_MINUTE: usize = 30;
const DEPOSIT_LOOKUPS_A_MINUTE_EACH: u32 = 6;
/// How long such a lookup may take before the wallet is asked to try again.
const DEPOSIT_LOOKUP_SECS: u64 = 25;
/// What a wallet is told when the same bitcoin is offered twice.
const DEPOSIT_TAKEN: &str = "this bitcoin deposit already stands behind another offer on the board";

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

    /// Whether one more look at the Bitcoin chain for a new buy offer's deposit fits the
    /// budget; counts it if so.
    fn deposit_lookup_allowed(&mut self, client: Option<IpAddr>, t: u64) -> bool {
        while self.deposit_lookups.front().is_some_and(|at| at + 60 <= t) {
            self.deposit_lookups.pop_front();
        }
        if self.deposit_lookups.len() >= DEPOSIT_LOOKUPS_A_MINUTE {
            return false;
        }
        let minute = t - t % 60;
        if let Some(c) = client {
            if self.deposit_lookups_of.len() > 20_000 {
                self.deposit_lookups_of.retain(|_, (m, _)| *m == minute);
            }
            let e = self.deposit_lookups_of.entry(c).or_insert((minute, 0));
            if e.0 != minute {
                *e = (minute, 0);
            }
            if e.1 >= DEPOSIT_LOOKUPS_A_MINUTE_EACH {
                return false;
            }
            e.1 += 1;
        }
        self.deposit_lookups.push_back(t);
        true
    }

    /// The deposits to look up again this round: at most `n` open buy offers (id, deposit
    /// address, sat), taking up where the last round stopped, so that every deposit has
    /// its turn however many there are.
    fn next_deposits(&mut self, n: usize) -> Vec<(String, String, u64)> {
        let mut all: Vec<(String, String, u64)> = self
            .deposits
            .iter()
            .filter_map(|(id, a)| self.offers.get(id).map(|o| (id.clone(), a.clone(), o.sats)))
            .collect();
        all.sort();
        if all.len() <= n {
            self.deposit_cursor = 0;
            return all;
        }
        let from = self.deposit_cursor % all.len();
        self.deposit_cursor = (from + n) % all.len();
        all.iter().cycle().skip(from).take(n).cloned().collect()
    }

    /// Takes in what a later look at the Bitcoin chain said about the deposit at `address`
    /// behind the buy offer `id`: `Some(true)` it is there, `Some(false)` it clearly is
    /// not, `None` the server did not answer. An offer whose bitcoin was clearly not there
    /// [`DEPOSIT_MISSES`] times in a row is taken down. Says whether the board changed.
    fn deposit_seen(&mut self, id: &str, address: &str, there: Option<bool>) -> bool {
        // The offer may have been taken, cancelled or listed anew while the lookup ran.
        if !self.offers.contains_key(id)
            || self.deposits.get(id).map(String::as_str) != Some(address)
        {
            return false;
        }
        match there {
            None => false,
            Some(true) => {
                self.deposit_misses.remove(id);
                false
            }
            Some(false) => {
                let n = self.deposit_misses.entry(id.to_string()).or_insert(0);
                *n += 1;
                if *n < DEPOSIT_MISSES {
                    return false;
                }
                self.take_down(id);
                true
            }
        }
    }

    /// Counts the promise `p` against the deposit at `address` (a request is counted once)
    /// and says what that deposit is promised to in all, and to how many swaps. A seller's
    /// wallet compares the sum with what the Bitcoin chain shows at the address: the same
    /// bitcoin shown to two sellers is seen by both. Bitcoin that stands behind an open buy
    /// offer is spoken for too: it counts against any promise that is not for that offer,
    /// so one address cannot back an offer and pay for a purchase at once.
    fn promise(&mut self, address: String, p: Promised) -> Result<(u64, usize), &'static str> {
        let p_offer = p.offer.clone();
        let known = self
            .promises
            .get(&address)
            .is_some_and(|v| v.iter().any(|x| x.id == p.id));
        if !known {
            let here = self.promises.get(&address).map_or(0, Vec::len);
            let of_offer = self
                .promises
                .values()
                .flatten()
                .filter(|x| x.offer == p.offer)
                .count();
            let total: usize = self.promises.values().map(Vec::len).sum();
            if here >= MAX_PROMISES_EACH
                || of_offer >= MAX_PROMISES_AN_OFFER
                || total >= MAX_PROMISES
            {
                return Err("too many promises; try again later");
            }
            self.promises.entry(address.clone()).or_default().push(p);
        }
        let v = &self.promises[&address];
        let offered = self
            .deposits
            .iter()
            .filter(|(id, a)| **a == address && **id != p_offer)
            .filter_map(|(id, _)| self.offers.get(id))
            .fold(0u64, |a, o| a.saturating_add(o.sats));
        Ok((
            v.iter().fold(offered, |a, x| a.saturating_add(x.sats)),
            v.len(),
        ))
    }

    /// Lists a new offer: the only way onto the board. A sell offer comes with its checked
    /// proof of funds, and its coins must not be behind anything else on the board. A buy
    /// offer comes with the deposit address its bitcoin was found at, and one deposit
    /// stands behind one offer.
    fn list(
        &mut self,
        o: Offer,
        backing: Option<Backing>,
        deposit: Option<String>,
        t: u64,
    ) -> Result<(), &'static str> {
        if self.offers.len() >= MAX_OFFERS {
            return Err("the board is full; try again later");
        }
        if self.offers.contains_key(&o.id)
            || self.pending.contains_key(&o.id)
            || self.coin_offers.contains_key(&o.id)
        {
            return Err("an offer with this id exists");
        }
        match o.side {
            Side::SellGnr => {
                let Some(b) = backing else {
                    return Err(board::RESERVE_MISSING);
                };
                if self.backs_another(&b) {
                    return Err("this GNR already backs another offer on the board");
                }
                self.backing.insert(o.id.clone(), b);
            }
            Side::BuyGnr => {
                let Some(a) = deposit else {
                    return Err(board::FUNDING_MISSING);
                };
                // Open offers and swaps in progress alike: a deposit on its way into a
                // swap's contract is not there for a second offer.
                if self.deposits.values().any(|x| *x == a) {
                    return Err(DEPOSIT_TAKEN);
                }
                self.deposits.insert(o.id.clone(), a);
            }
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
        self.deposit_misses.remove(id);
        self.auto.remove(id);
        self.part.remove(id);
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
    history: std::sync::OnceLock<crate::history::History>,
    /// The all-time median price (it reads the whole history): (swaps then, when, median).
    /// The all-time medians (every swap, verified ones) and what they were read at: the
    /// number of swaps, how many of them were verified, and when.
    median_all: Mutex<Option<MedianAll>>,
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
/// A board file read back. One that exists but cannot be read is moved aside (and
/// logged), never written over by the empty state that replaces it.
fn read_json<T: serde::de::DeserializeOwned>(p: &std::path::Path) -> Option<T> {
    let b = std::fs::read(p).ok()?;
    match serde_json::from_slice(&b) {
        Ok(v) => Some(v),
        Err(e) => {
            let aside = p.with_extension(format!("unreadable-{}", now()));
            tracing::error!(
                "{} cannot be read ({e}); kept as {}",
                p.display(),
                aside.display()
            );
            let _ = std::fs::rename(p, &aside);
            None
        }
    }
}

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
        // Whatever put it there, an offer stays only with what stands behind it: the
        // proven GNR of a sell offer, the found bitcoin of a buy offer.
        let (backing, deposits) = (&i.backing, &i.deposits);
        i.offers.retain(|id, o| match o.side {
            Side::SellGnr => backing.contains_key(id),
            Side::BuyGnr => deposits.contains_key(id),
        });
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
        i.deposit_misses.retain(|id, _| offers.contains_key(id));
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
        i.sent_bytes.retain(|_, (d, _)| *d == day);
        // Completed swaps stay on disk (the trade history); memory keeps what the price
        // needs, and without the history every swap up to the cap.
        if i.hist_ok && i.trades.len() > MEMORY_TRADES_MIN {
            let keep_from = i.trades.len() - MEMORY_TRADES_MIN;
            let old = i.trades[..keep_from]
                .iter()
                .take_while(|(_, at)| at + MEMORY_TRADE_SECS <= t)
                .count();
            if old > 0 {
                i.trades.drain(..old);
            }
        }
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
        if let Some(t) = read_json::<Vec<(Trade, u64)>>(&p) {
            i.trades = t;
        }
        if let Some(t) =
            read_json::<Vec<(CoinTrade, u64)>>(&p.with_file_name("swap-coin-trades.json"))
        {
            i.coin_trades = t;
        }
        if let Some(v) = read_json::<SavedPaid>(&p.with_file_name("swap-trades-paid.json")) {
            i.paid = v.paid.into_iter().map(|(h, s, k)| (h, (s, k))).collect();
            i.disputed = v.disputed.into_iter().collect();
            i.claims = v.claims.into_iter().collect();
        }
        // Offers and sealed messages survive restarts too (a node update must not drop
        // them, or a swap in progress could lose a code).
        let mut purged = false;
        if let Some(s) = read_json::<Saved>(&p.with_file_name("swap-board.json")) {
            // Offers from before sell offers had to prove their GNR are not carried over.
            // From a file written before buy offers had to have their bitcoin deposited,
            // the buy offers without a found deposit go (`expire` below; their makers'
            // wallets see it in their next ping). Mailboxes always stay, and swaps in
            // progress as far as the board can stand for them: a running swap must not
            // lose a code.
            purged = s.rules < RULES;
            if s.rules >= RULES_READ {
                i.offers = s.offers.into_iter().map(|o| (o.id.clone(), o)).collect();
                i.backing = s.backing.into_iter().collect();
                i.deposits = s.deposits.into_iter().collect();
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
        self.open_history(&p, i);
        Board::expire(i, now());
        // An offer without what stands behind it cannot be on the board, whatever the
        // file said (`expire` has dropped those). Nor can a sell swap in progress carried
        // over from before sell offers proved their GNR: the board cannot vouch for it,
        // and nothing would keep its coins from being listed again. Such a swap itself
        // goes on: it needs only the mailboxes.
        let backing = &i.backing;
        i.pending
            .retain(|id, (o, _)| o.side != Side::SellGnr || backing.contains_key(id));
        if purged {
            self.save_board(i);
        }
    }

    /// Opens the history on disk next to the board's files. The first time, every swap the
    /// files hold is imported (the files stay as they were, with a copy of the swaps from
    /// before 1.0.53). Without it the board keeps working from memory, as before 1.0.53.
    fn open_history(&self, p: &std::path::Path, i: &mut Inner) {
        let h = match crate::history::History::open(&p.with_file_name("swap-history.redb")) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("trade history unavailable, keeping it in memory: {e}");
                return;
            }
        };
        // Swaps the files have and the history not (it was written last before a crash):
        // written with the next flush.
        if h.count().unwrap_or(0) > 0 {
            let missing: Vec<(Trade, u64)> = i
                .trades
                .iter()
                .filter(|(x, _)| hash32(&x.hash).is_some_and(|k| !h.contains(&k).unwrap_or(true)))
                .cloned()
                .collect();
            i.hist_new.extend(missing);
        } else if !i.trades.is_empty() {
            let backup = p.with_extension("json.pre-1.0.53");
            if !backup.exists() {
                let _ = std::fs::copy(p, &backup);
            }
            let recs: Vec<crate::history::Rec> = i
                .trades
                .iter()
                .filter_map(|(x, at)| hist_rec(i, x, *at))
                .collect();
            if let Err(e) = h.write(&recs, &[]) {
                tracing::error!("importing the trade history failed, keeping it in memory: {e}");
                return;
            }
            tracing::info!(swaps = recs.len(), "trade history imported");
        }
        i.hist_ok = true;
        let _ = self.history.set(h);
    }

    /// Whether the history on disk has the swap `hash` (one the memory may have let go).
    fn in_history(&self, hash: &str) -> bool {
        let (Some(h), Some(k)) = (self.history.get(), hash32(hash)) else {
            return false;
        };
        h.contains(&k).unwrap_or(false)
    }

    /// Writes new swaps and changed statuses to the history (from `flush`, off the
    /// requests). What fails to be written is tried again next time.
    fn flush_history(&self) {
        let Some(h) = self.history.get() else {
            return;
        };
        let (new, touch, recs, updates) = {
            let mut i = self.inner.lock();
            if i.hist_new.is_empty() && i.hist_touch.is_empty() {
                return;
            }
            let new = std::mem::take(&mut i.hist_new);
            let touch = std::mem::take(&mut i.hist_touch);
            let recs: Vec<crate::history::Rec> = new
                .iter()
                .filter_map(|(x, at)| hist_rec(&i, x, *at))
                .collect();
            let updates: Vec<([u8; 32], crate::history::Status)> = touch
                .iter()
                .filter_map(|hash| Some((hash32(hash)?, hist_status(&i, hash))))
                .collect();
            (new, touch, recs, updates)
        };
        if let Err(e) = h.write(&recs, &updates) {
            tracing::error!("writing the trade history failed (tried again): {e}");
            let mut i = self.inner.lock();
            let mut back = new;
            back.append(&mut i.hist_new);
            i.hist_new = back;
            i.hist_touch.extend(touch);
        } else if !recs.is_empty() || !updates.is_empty() {
            self.changed();
        }
    }

    /// On disk before it replaces the old file, so a crash leaves one or the other.
    fn write(path: &std::path::Path, b: Vec<u8>) {
        use std::io::Write;
        let tmp = path.with_extension("tmp");
        let ok = std::fs::File::create(&tmp)
            .and_then(|mut f| f.write_all(&b).and_then(|()| f.sync_all()))
            .is_ok();
        if ok {
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
        self.flush_history();
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
    let fresh: Vec<String> = found
        .iter()
        .filter(|(h, _)| known.contains(h) && !i.claims.contains_key(h))
        .map(|(h, _)| h.clone())
        .collect();
    let of_a_trade = !fresh.is_empty();
    i.claims.extend(found);
    i.hist_touch.extend(fresh);
    if i.scanned.is_none_or(|s| s < tip) {
        i.scanned = Some(tip);
    }
    if of_a_trade {
        node.board.save(&i);
    }
}

/// A swap for the history, with its status as the board knows it.
fn hist_rec(i: &Inner, x: &Trade, at: u64) -> Option<crate::history::Rec> {
    let st = hist_status(i, &x.hash);
    Some(crate::history::Rec {
        at,
        hash: hash32(&x.hash)?,
        gnr: x.gnr,
        sats: x.sats,
        verified: st.verified,
        paid: st.paid,
        disputed: st.disputed,
        claim: st.claim,
    })
}

fn hist_status(i: &Inner, hash: &str) -> crate::history::Status {
    crate::history::Status {
        amounts: i
            .trades
            .iter()
            .rev()
            .find(|(x, _)| x.hash == hash)
            .map(|(x, _)| (x.gnr, x.sats)),
        verified: i.paid.contains_key(hash),
        paid: i.paid.get(hash).map_or(0, |p| p.0),
        disputed: i.disputed.contains(hash),
        claim: i.claims.get(hash).and_then(|c| {
            Some(crate::history::ClaimRec {
                height: c.height,
                time: c.time,
                refund_height: c.refund_height,
                txid: hash32(&c.txid)?,
                secret: hash32(&c.secret)?,
            })
        }),
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
            // Every seller locks GNR only for a buyer whose bitcoin is deposited (wallets
            // before 1.0.56 read this to know that they must pay from a deposit).
            x["prepaid"] = json!(o.side == Side::SellGnr);
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
    // What is on offer in all. Every open offer is covered, so these are sums of coins
    // that exist: GNR proven by its sellers, bitcoin found at its buyers' deposits.
    let total = |side: Side, f: fn(&Offer) -> u64| {
        i.offers
            .values()
            .filter(|o| o.side == side)
            .map(f)
            .fold(0u64, u64::saturating_add)
    };
    serde_json::to_vec(&json!({
        "offers": v, "in_progress": p, "now": now(),
        "for_sale_gnr": total(Side::SellGnr, |o| o.gnr),
        "wanted_sats": total(Side::BuyGnr, |o| o.sats),
        "issued": node.chain.supply(),
    }))
    .unwrap_or_default()
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
    /// A buy offer: the proof of the deposit address its bitcoin is at
    /// (`krypton_swap::board::check_funding`).
    #[serde(default)]
    funding: Option<board::Funding>,
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
        // Its zk-STARK proof is checked with the transactions' proofs: a few at a time.
        let Ok(Ok(_verifying)) =
            tokio::time::timeout(Duration::from_secs(10), node.tx_verify.acquire()).await
        else {
            return bad("the board is busy checking proofs; try again in a moment");
        };
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
            Err(_) => return bad("the proof of funds could not be checked; try again"),
        }
    } else {
        None
    };
    // A buy offer is listed only with its bitcoin at a deposit address of the buyer's own
    // wallet: the wallet proves the address is its own, and this board finds the offer's
    // amount there on the Bitcoin chain, confirmed and unspent. A board that cannot read
    // the Bitcoin chain takes no buy offers, and a lookup that fails lists nothing.
    let deposit = if o.side == Side::BuyGnr {
        let Some(f) = &p.funding else {
            return bad(board::FUNDING_MISSING);
        };
        let address = match board::check_funding(&o, f) {
            Ok(a) => a,
            Err(e) => return bad(e),
        };
        let Some(api) = btc_api() else {
            return bad("this board cannot read the Bitcoin chain, so it lists no buy offers");
        };
        let known = address.to_string();
        // What needs no lookup is refused before one is spent on it.
        let refused = off_runtime(|| {
            let mut i = node.board.inner.lock();
            if i.deposits.values().any(|x| *x == known) {
                return Some(DEPOSIT_TAKEN);
            }
            (!i.deposit_lookup_allowed(client, now()))
                .then_some("the board is busy looking deposits up; try again in a moment")
        });
        if let Some(why) = refused {
            return bad(why);
        }
        let need = o.sats;
        let looked = tokio::time::timeout(
            Duration::from_secs(DEPOSIT_LOOKUP_SECS),
            tokio::task::spawn_blocking(move || deposit_at(&api, &address, need)),
        )
        .await;
        match looked {
            Ok(Ok(Ok(true))) => Some(known),
            Ok(Ok(Ok(false))) => {
                return bad(
                    "the bitcoin of this offer is not at its deposit address, confirmed and unspent",
                );
            }
            _ => {
                return bad("the board could not reach its Bitcoin server; try again in a moment");
            }
        }
    } else {
        None
    };
    // Checked and listed in one step, under the board's lock: of two offers posted at the
    // same moment with the same coins behind them, only the first is listed.
    off_runtime(move || list_offer(&node, client, o, p.auto, backing, deposit))
}

fn list_offer(
    node: &Arc<Node>,
    client: Option<IpAddr>,
    o: Offer,
    auto: Option<board::Auto>,
    backing: Option<Backing>,
    deposit: Option<String>,
) -> Response {
    let mut i = node.board.inner.lock();
    node.board.load(&mut i);
    let t = now();
    Board::expire(&mut i, t);
    let (id, buying) = (o.id.clone(), o.side == Side::BuyGnr);
    if let Err(e) = i.list(o, backing, deposit, t) {
        return bad(e);
    }
    if let Some(c) = client {
        i.lister.insert(id.clone(), c);
    }
    if let Some(a) = auto {
        i.auto.insert(id, a);
    }
    node.board.save_board(&i);
    Json(json!({"ok": true, "deposited": buying})).into_response()
}

/// The Bitcoin server this board reads deposits of buy offers from (`KRYPTON_BTC_API`:
/// an Esplora URL, or `rpc:` and a Bitcoin Core URL). Without it the board lists no buy
/// offers: it could not tell whether their bitcoin exists.
fn btc_api() -> Option<String> {
    std::env::var("KRYPTON_BTC_API")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether `coins` (the unspent outputs of an address) hold at least `need` sat in blocks:
/// bitcoin that is still on its way does not count.
fn covered(coins: &[krypton_swap::btc::Utxo], need: u64) -> bool {
    coins
        .iter()
        .filter(|u| u.height.is_some())
        .fold(0u64, |a, u| a.saturating_add(u.value.to_sat()))
        >= need
}

/// Whether confirmed, unspent outputs worth at least `need` sat pay `address`.
fn deposit_at(
    api: &str,
    address: &krypton_swap::bitcoin::Address,
    need: u64,
) -> anyhow::Result<bool> {
    let btc = krypton_swap::btc::connect(api)?;
    Ok(covered(&btc.unspent(address)?, need))
}

/// How often deposits behind open buy offers are looked up again, how many each time
/// (they take turns), and how many times in a row a deposit must be clearly missing
/// before its offer is taken down: one odd answer of a Bitcoin server takes nothing down.
const DEPOSIT_CHECK_SECS: u64 = 120;
const DEPOSIT_CHECK_BATCH: usize = 60;
const DEPOSIT_MISSES: u8 = 2;

/// Looks the deposits of open buy offers up again and takes down the offers whose bitcoin
/// has moved: a buy offer says its bitcoin is there, and this keeps it true. The lookups
/// run off the board's lock and off the request that started them.
fn recheck_deposits(node: &Arc<Node>) {
    let Some(api) = btc_api() else { return };
    let list = {
        let mut i = node.board.inner.lock();
        let t = now();
        // (A private test chain has its blocks seconds apart.)
        let every = if node.chain.params.network == krypton_core::Network::Devnet {
            15
        } else {
            DEPOSIT_CHECK_SECS
        };
        if i.deposits_checked + every > t {
            return;
        }
        i.deposits_checked = t;
        i.next_deposits(DEPOSIT_CHECK_BATCH)
    };
    if list.is_empty() {
        return;
    }
    let node = node.clone();
    tokio::task::spawn_blocking(move || {
        use std::str::FromStr;
        // Only a clear "not there" counts against an offer, never a failed lookup.
        let seen: Vec<(String, String, Option<bool>)> = list
            .into_iter()
            .map(|(id, a, need)| {
                let there = krypton_swap::bitcoin::Address::from_str(&a)
                    .ok()
                    .map(|x| x.assume_checked())
                    .and_then(|x| deposit_at(&api, &x, need).ok());
                (id, a, there)
            })
            .collect();
        let mut i = node.board.inner.lock();
        let mut changed = false;
        for (id, a, there) in &seen {
            changed |= i.deposit_seen(id, a, *there);
        }
        if changed {
            node.board.save_board(&i);
        }
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
        let day = t - t % 86_400;
        let bytes = i.sent_bytes.entry(c).or_insert((day, 0));
        if bytes.0 != day {
            *bytes = (day, 0);
        }
        if all >= MAX_MESSAGES_A_DAY_EACH
            || to >= MAX_MESSAGES_A_DAY_EACH_INBOX
            || bytes.1 + e.data.len() as u64 > MAX_MESSAGE_BYTES_A_DAY_EACH
        {
            return bad("too many messages from your address today; try again tomorrow");
        }
    }
    if i.messages >= MAX_MESSAGES || i.message_bytes + e.data.len() > MAX_MESSAGE_BYTES_TOTAL {
        return bad("the board's message relay is full; try again later");
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
        i.sent_bytes.entry(c).or_insert((0, 0)).1 += len as u64;
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
                // What went wrong stays in the node's log: it can name the Bitcoin server.
                let sats = paid_on_chain(&api, &t, p, &secret).map_err(|e| {
                    tracing::warn!("looking up the bitcoin of swap {}: {e}", t.hash);
                    "the bitcoin of this swap could not be looked up right now; the report is sent again by itself".to_string()
                })?;
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
        let on_disk = n.board.in_history(&t.hash);
        let mut i = n.board.inner.lock();
        let at = i.trades.iter().position(|(x, _)| x.hash == t.hash);
        if at.is_none() && (on_disk || i.hist_new.iter().any(|(x, _)| x.hash == t.hash)) {
            // Recorded long ago: never a second time, and its status no longer changes.
            return Ok(json!({"ok": true, "added": false, "verified": false}));
        }
        let mut added = false;
        match (found, at) {
            (Some(f), Some(k)) => {
                let same = i.trades[k].0.gnr == t.gnr && i.trades[k].0.sats == t.sats;
                if i.paid.contains_key(&t.hash) {
                    // The swap's other wallet says something else: neither counts.
                    if !same {
                        i.disputed.insert(t.hash.clone());
                        i.hist_touch.insert(t.hash.clone());
                    }
                } else {
                    // The first proven report stands in for an unproven one.
                    if !same {
                        i.trades[k].0 = t.clone();
                    }
                    i.paid.insert(t.hash.clone(), f);
                    i.hist_touch.insert(t.hash.clone());
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
            let at = now();
            i.trades.push((t.clone(), at));
            if i.hist_ok {
                i.hist_new.push((t.clone(), at));
            }
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

/// The price assessment, computed again at most once a minute or when the swaps or their
/// verification change (it rebuilds a week of prices hour by hour).
struct PriceCache(parking_lot::Mutex<Option<(usize, usize, u64, krypton_swap::price::Assessment)>>);
static PRICE_CACHE: PriceCache = PriceCache(parking_lot::Mutex::new(None));

impl PriceCache {
    fn with_cache(
        &self,
        i: &Inner,
        verified: &HashMap<String, u64>,
        t: u64,
    ) -> krypton_swap::price::Assessment {
        let key = (i.trades.len(), verified.len(), t / 60);
        let mut c = self.0.lock();
        if let Some((n, v, m, a)) = c.as_ref()
            && (*n, *v, *m) == key
        {
            return a.clone();
        }
        let a = krypton_swap::price::assess(&board::price_swaps(&i.trades, verified), t);
        *c = Some((key.0, key.1, key.2, a.clone()));
        a
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
    let verified = i.verified();
    let a = PRICE_CACHE.with_cache(i, &verified, t);
    board::market_with(&i.trades, &verified, &offers, &pending, t, &a)
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
        // Its bitcoin was found on the Bitcoin chain, paid through the swap's contract, and
        // how much of it (what the price weighs the swap by).
        "verified": i.paid.contains_key(&x.hash) && !i.disputed.contains(&x.hash),
        "paid_sats": i.paid.get(&x.hash).filter(|_| !i.disputed.contains(&x.hash)).map_or(0, |p| p.0),
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
    /// How many of the newest matching swaps to skip (pages of `limit`).
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
    /// Only swaps reported at or after this time.
    #[serde(default)]
    since: Option<u64>,
    /// Only swaps whose bitcoin the board found on the Bitcoin chain.
    #[serde(default)]
    verified: bool,
}

/// Every completed swap, newest first, a page at a time (1.0.53): the trade history.
pub async fn history(State(node): S, Query(q): Query<HistoryQuery>) -> Response {
    if q.offset > HISTORY_OFFSET_MAX {
        return bad("offset too large: narrow the history with since");
    }
    off_runtime(|| {
        let limit = q.limit.unwrap_or(50).clamp(1, HISTORY_PAGE_MAX);
        let since = q.since.unwrap_or(0);
        let tip = node.chain.tip().height();
        node.board.load(&mut node.board.inner.lock());
        if let Some(h) = node.board.history.get() {
            return match h.page(q.offset, limit, since, q.verified) {
                Ok((recs, total)) => {
                    let page: Vec<Value> = recs.iter().map(|r| rec_json(r, tip)).collect();
                    let more = (q.offset.saturating_add(page.len()) as u64) < total;
                    Json(json!({"swaps": page, "offset": q.offset, "more": more, "total": total, "now": now()}))
                        .into_response()
                }
                Err(e) => {
                    tracing::error!("trade history: {e}");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error": "the trade history is not available right now"})),
                    )
                        .into_response()
                }
            };
        }
        let i = node.board.inner.lock();
        let shown = |x: &Trade, at: u64| {
            at >= since
                && (!q.verified || (i.paid.contains_key(&x.hash) && !i.disputed.contains(&x.hash)))
        };
        let mut total = 0usize;
        let mut page: Vec<Value> = Vec::new();
        for (x, at) in i.trades.iter().rev() {
            // Reported in time order: nothing older can match.
            if *at < since {
                break;
            }
            if !shown(x, *at) {
                continue;
            }
            if total >= q.offset && page.len() < limit {
                page.push(trade_json(&i, x, *at, tip));
            }
            total += 1;
        }
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
    if let Some(h) = node.board.history.get() {
        // The last 24 hours are in memory; everything else comes from the days on disk.
        let t = now();
        let last24: Vec<Done> = i
            .trades
            .iter()
            .filter(|(x, at)| at + 86_400 > t && !i.disputed.contains(&x.hash) && x.gnr > 0)
            .map(|(x, at)| (*at, x.price(), x.gnr, x.sats, i.paid.contains_key(&x.hash)))
            .collect();
        drop(i);
        match store_summary(node, h, &last24, t) {
            Ok(v) => return serde_json::to_vec(&v).unwrap_or_default(),
            Err(e) => tracing::error!("trade history: {e}"),
        }
        i = node.board.inner.lock();
    }
    let list: Vec<Done> = i
        .trades
        .iter()
        .filter(|(x, _)| !i.disputed.contains(&x.hash) && x.gnr > 0)
        .map(|(x, at)| (*at, x.price(), x.gnr, x.sats, i.paid.contains_key(&x.hash)))
        .collect();
    drop(i);
    serde_json::to_vec(&history_figures(&list, now())).unwrap_or_default()
}

/// One swap of the history on disk as the history API shows it.
fn rec_json(r: &crate::history::Rec, tip: u64) -> Value {
    let c = r.claim.as_ref().map(|c| {
        json!({
            "height": c.height, "time": c.time, "txid": hex::encode(c.txid),
            "secret": hex::encode(c.secret), "refund_height": c.refund_height,
        })
    });
    json!({
        "at": r.at, "gnr": r.gnr, "sats": r.sats, "price": r.price(), "hash": hex::encode(r.hash),
        "verified": r.verified && !r.disputed,
        "paid_sats": if r.disputed { 0 } else { r.paid },
        "claim": c,
        "confirmations": r.claim.as_ref().map(|c| (tip + 1).saturating_sub(c.height)),
    })
}

/// The summary from the history on disk: totals from the days' figures. The all-time
/// medians read the whole history, so they are kept: for a minute while swaps arrive, and
/// for ten while nothing changes.
fn store_summary(
    node: &Arc<Node>,
    h: &crate::history::History,
    last24: &[Done],
    t: u64,
) -> anyhow::Result<Value> {
    let days = h.days()?;
    let count = h.count()?;
    let sum =
        |f: fn(&crate::history::DayFig) -> u64| days.iter().map(f).fold(0u64, u64::saturating_add);
    let verified = sum(|d| d.verified);
    let (median, v_median) = {
        let mut m = node.board.median_all.lock();
        match *m {
            Some((n, nv, at, a, v))
                if at.elapsed() < Duration::from_secs(60)
                    || ((n, nv) == (count, verified)
                        && at.elapsed() < Duration::from_secs(600)) =>
            {
                (a, v)
            }
            _ => {
                let (a, v) = h.median_all()?;
                *m = Some((count, verified, Instant::now(), a, v));
                (a, v)
            }
        }
    };
    let (gnr, sats) = (sum(|d| d.gnr), sum(|d| d.sats));
    let (v_gnr, v_sats) = (sum(|d| d.v_gnr), sum(|d| d.v_sats));
    let least = |f: fn(&crate::history::DayFig) -> Option<f64>| {
        days.iter()
            .filter_map(f)
            .fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.min(x))))
    };
    let most = |f: fn(&crate::history::DayFig) -> Option<f64>| {
        days.iter()
            .filter_map(f)
            .fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.max(x))))
    };
    let vwap = |gnr: u64, sats: u64| (gnr > 0).then(|| sats as f64 / (gnr as f64 / 1e8));
    let span = h.span()?;
    let all = json!({
        "swaps": sum(|d| d.swaps),
        "verified": verified,
        "gnr": gnr,
        "sats": sats,
        "low": least(|d| d.low),
        "high": most(|d| d.high),
        "median": median,
        "vwap": vwap(gnr, sats),
        "first": span.map(|s| s.0),
        "last": span.map(|s| s.1),
        // The same over the verified swaps alone (1.0.55): what the charts and totals of the
        // trade history show first, since a made-up report cannot move them.
        "verified_figures": {
            "gnr": v_gnr, "sats": v_sats, "low": least(|d| d.v_low), "high": most(|d| d.v_high),
            "median": v_median, "vwap": vwap(v_gnr, v_sats),
        },
    });
    let day = history_figures(last24, t)["all"].clone();
    let days: Vec<Value> = days
        .iter()
        .filter(|d| d.swaps > 0)
        .map(|d| {
            json!({
                "day": d.day, "swaps": d.swaps, "verified": d.verified, "gnr": d.gnr, "sats": d.sats,
                "low": d.low, "high": d.high, "median": d.median, "vwap": d.vwap,
                "verified_figures": {
                    "gnr": d.v_gnr, "sats": d.v_sats, "low": d.v_low, "high": d.v_high,
                    "median": d.v_median, "vwap": d.v_vwap,
                },
            })
        })
        .collect();
    Ok(json!({"all": all, "day": day, "days": days, "now": t}))
}

/// A completed swap for the figures: (time, price, GNR, sat, verified).
type Done = (u64, f64, u64, u64, bool);

/// The all-time medians as they were last read: (swaps, verified swaps, when, the median of
/// every swap, the median of the verified ones).
type MedianAll = (u64, u64, Instant, Option<f64>, Option<f64>);

/// Totals and per-day figures of completed swaps.
fn history_figures(list: &[Done], t: u64) -> Value {
    /// GNR, bitcoin, low, high, median and the average weighed by the bitcoin moved.
    fn core(v: &[&Done]) -> Value {
        let mut prices: Vec<f64> = v.iter().map(|x| x.1).collect();
        prices.sort_by(f64::total_cmp);
        let gnr: u64 = v.iter().map(|x| x.2).fold(0, u64::saturating_add);
        let sats: u64 = v.iter().map(|x| x.3).fold(0, u64::saturating_add);
        json!({
            "gnr": gnr,
            "sats": sats,
            "low": prices.first(),
            "high": prices.last(),
            "median": crate::history::median(&prices),
            // What a GNR cost on average, weighed by the bitcoin moved.
            "vwap": if gnr > 0 { Some(sats as f64 / (gnr as f64 / 1e8)) } else { None },
        })
    }
    fn figures(v: &[Done]) -> Value {
        let all: Vec<&Done> = v.iter().collect();
        let verified: Vec<&Done> = v.iter().filter(|x| x.4).collect();
        let mut f = core(&all);
        f["swaps"] = json!(v.len());
        f["verified"] = json!(verified.len());
        // The same over the verified swaps alone (1.0.55).
        f["verified_figures"] = core(&verified);
        f
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
    let made = Promised {
        id: p.id,
        offer: r.offer,
        sats: p.sats,
        release: p.release,
        at: t,
    };
    let (sum, n) = match i.promise(address, made) {
        Ok(x) => x,
        Err(e) => return bad(e),
    };
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

    fn at(a: &str) -> Option<String> {
        Some(a.to_string())
    }

    fn coin(sats: u64, height: Option<u32>) -> krypton_swap::btc::Utxo {
        use krypton_swap::bitcoin::{Amount, OutPoint};
        krypton_swap::btc::Utxo {
            outpoint: OutPoint::null(),
            value: Amount::from_sat(sats),
            height,
        }
    }

    /// The one way onto the board: a sell offer with its proven GNR, a buy offer with its
    /// found bitcoin. Neither stands in for the other.
    #[test]
    fn an_offer_is_listed_only_with_what_stands_behind_it() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), None, None, T),
            Err(board::RESERVE_MISSING)
        );
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), None, None, T),
            Err(board::FUNDING_MISSING)
        );
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), backing("n1", "d1"), None, T),
            Err(board::FUNDING_MISSING)
        );
        assert_eq!(
            i.list(offer("a", Side::SellGnr), None, at("addr1"), T),
            Err(board::RESERVE_MISSING)
        );
        assert!(i.offers.is_empty() && i.seen.is_empty());
        assert!(i.backing.is_empty() && i.deposits.is_empty());
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), None, at("addr1"), T),
            Ok(())
        );
        assert!(i.offers.contains_key("b") && i.backing.is_empty());
        assert_eq!(i.deposits.get("b").map(String::as_str), Some("addr1"));
        // An offer that got onto the board any other way does not stay there.
        i.offers.insert("x".into(), offer("x", Side::BuyGnr));
        i.offers.insert("y".into(), offer("y", Side::SellGnr));
        i.seen.insert("x".into(), T);
        i.seen.insert("y".into(), T);
        Board::expire(&mut i, T + 1);
        assert_eq!(i.offers.keys().collect::<Vec<_>>(), ["b"]);
    }

    /// The same bitcoin cannot be offered twice: not while its offer is open, and not
    /// while the swap it was taken for runs.
    #[test]
    fn one_deposit_stands_behind_one_offer() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), None, at("addr1"), T),
            Ok(())
        );
        assert_eq!(
            i.list(offer("c", Side::BuyGnr), None, at("addr1"), T),
            Err(DEPOSIT_TAKEN)
        );
        assert!(!i.offers.contains_key("c") && !i.deposits.contains_key("c"));
        i.start("b", T);
        Board::expire(&mut i, T + 60);
        assert!(i.pending.contains_key("b") && i.deposits.contains_key("b"));
        assert_eq!(
            i.list(offer("c", Side::BuyGnr), None, at("addr1"), T),
            Err(DEPOSIT_TAKEN)
        );
        i.take_down("b");
        assert!(i.deposits.is_empty());
        assert_eq!(
            i.list(offer("c", Side::BuyGnr), None, at("addr1"), T),
            Ok(())
        );
        assert_eq!(
            i.list(offer("d", Side::BuyGnr), None, at("addr2"), T),
            Ok(())
        );
        assert_eq!(i.deposits.len(), 2);
    }

    /// A buy offer says its bitcoin is there; when it has clearly gone, the offer goes.
    /// A Bitcoin server that does not answer, or answers oddly once, takes nothing down.
    #[test]
    fn a_buy_offer_goes_when_its_bitcoin_has_gone() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), None, at("addr1"), T),
            Ok(())
        );
        assert_eq!(
            i.list(offer("c", Side::BuyGnr), None, at("addr2"), T),
            Ok(())
        );
        assert!(!i.deposit_seen("b", "addr1", None));
        assert!(!i.deposit_seen("b", "addr1", Some(false)));
        assert!(i.offers.contains_key("b"), "one miss is not enough");
        assert!(!i.deposit_seen("b", "addr1", Some(true)));
        assert!(!i.deposit_seen("b", "addr1", Some(false)));
        assert!(
            i.offers.contains_key("b"),
            "found in between, the count starts over"
        );
        assert!(!i.deposit_seen("b", "addr1", None));
        assert!(i.deposit_seen("b", "addr1", Some(false)));
        assert!(!i.offers.contains_key("b") && !i.deposits.contains_key("b"));
        assert!(!i.seen.contains_key("b") && i.deposit_misses.is_empty());
        assert!(i.offers.contains_key("c"), "the other offer is not touched");
        // An answer about a deposit the offer no longer has is not used.
        for _ in 0..3 {
            assert!(!i.deposit_seen("c", "addr1", Some(false)));
            assert!(!i.deposit_seen("gone", "addr2", Some(false)));
        }
        assert!(i.offers.contains_key("c") && i.deposit_misses.is_empty());
        // A swap in progress is not judged by its deposit: that bitcoin is on its way into
        // the swap's contract.
        i.start("c", T);
        for _ in 0..3 {
            assert!(!i.deposit_seen("c", "addr2", Some(false)));
        }
        assert!(i.pending.contains_key("c") && i.deposits.contains_key("c"));
        // The address that was given up can stand behind a new offer.
        assert_eq!(
            i.list(offer("e", Side::BuyGnr), None, at("addr1"), T),
            Ok(())
        );
    }

    /// One deposit shown to two sellers: the board counts both promises against it, and
    /// each seller sees a sum the Bitcoin chain does not cover, so neither locks twice
    /// for the same bitcoin.
    #[test]
    fn a_deposit_promised_to_two_sellers_shows_both() {
        let p = |id: &str, offer: &str, sats: u64| Promised {
            id: id.into(),
            offer: offer.into(),
            sats,
            release: "r".into(),
            at: T,
        };
        let mut i = Inner::default();
        assert_eq!(
            i.promise("addr1".into(), p("r1", "o1", 50_000)),
            Ok((50_000, 1))
        );
        assert_eq!(
            i.promise("addr1".into(), p("r1", "o1", 50_000)),
            Ok((50_000, 1)),
            "the same request is counted once"
        );
        assert_eq!(
            i.promise("addr1".into(), p("r2", "o2", 50_000)),
            Ok((100_000, 2)),
            "a second seller sees twice what the deposit holds"
        );
        assert_eq!(
            i.promise("addr2".into(), p("r3", "o3", 50_000)),
            Ok((50_000, 1))
        );
        // No offer gathers more promises than it could ever need.
        for n in 0..MAX_PROMISES_AN_OFFER {
            assert!(
                i.promise(format!("a{n}"), p(&format!("x{n}"), "o9", 10_000))
                    .is_ok()
            );
        }
        assert!(i.promise("z".into(), p("y", "o9", 10_000)).is_err());
        assert!(
            !i.promises.contains_key("z"),
            "a refused promise leaves nothing behind"
        );
        // Bitcoin behind an open buy offer is spoken for: it does not also pay for a
        // purchase made from the same address. The offer's own swap is another matter.
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), None, at("addr5"), T),
            Ok(())
        );
        assert_eq!(
            i.promise("addr5".into(), p("r7", "other", 20_000)),
            Ok((70_000, 1)),
            "the offer's 50,000 sat and the purchase's 20,000"
        );
        assert_eq!(
            i.promise("addr5".into(), p("r8", "b", 50_000)),
            Ok((70_000, 2))
        );
        i.take_down("b");
        assert_eq!(
            i.promise("addr5".into(), p("r7", "other", 20_000)),
            Ok((70_000, 2))
        );
        // Promises run out by themselves (a swap that never paid frees the deposit).
        Board::expire(&mut i, T + board::PROMISE_SECS + 1);
        assert!(i.promises.is_empty());
    }

    #[test]
    fn every_deposit_has_its_turn() {
        let mut i = Inner::default();
        for n in 0..7 {
            let o = offer(&format!("b{n}"), Side::BuyGnr);
            assert_eq!(i.list(o, None, at(&format!("addr{n}")), T), Ok(()));
        }
        let ids = |v: Vec<(String, String, u64)>| v.into_iter().map(|x| x.0).collect::<Vec<_>>();
        assert_eq!(ids(i.next_deposits(3)), ["b0", "b1", "b2"]);
        assert_eq!(ids(i.next_deposits(3)), ["b3", "b4", "b5"]);
        assert_eq!(ids(i.next_deposits(3)), ["b6", "b0", "b1"]);
        assert_eq!(ids(i.next_deposits(3)), ["b2", "b3", "b4"]);
        // Few enough: all of them every time, with the address and the amount to find.
        let all = i.next_deposits(60);
        assert_eq!(all.len(), 7);
        assert_eq!(all[2], ("b2".to_string(), "addr2".to_string(), 50_000));
        // Swaps in progress are not looked up.
        i.start("b0", T);
        assert_eq!(i.next_deposits(60).len(), 6);
        assert!(Inner::default().next_deposits(60).is_empty());
    }

    #[test]
    fn bitcoin_on_its_way_does_not_count() {
        assert!(!covered(&[], 1));
        assert!(!covered(&[coin(50_000, None)], 50_000));
        assert!(covered(&[coin(50_000, Some(100))], 50_000));
        assert!(!covered(&[coin(49_999, Some(100))], 50_000));
        assert!(!covered(
            &[coin(30_000, Some(100)), coin(30_000, None)],
            50_000
        ));
        assert!(covered(
            &[coin(30_000, Some(100)), coin(20_000, Some(101))],
            50_000
        ));
        // Amounts no chain holds do not wrap around.
        assert!(covered(
            &[coin(u64::MAX, Some(1)), coin(u64::MAX, Some(1))],
            u64::MAX
        ));
    }

    #[test]
    fn deposit_lookups_are_rationed() {
        let ip = |n: u8| Some(IpAddr::from([10, 0, 0, n]));
        let mut i = Inner::default();
        for _ in 0..DEPOSIT_LOOKUPS_A_MINUTE_EACH {
            assert!(i.deposit_lookup_allowed(ip(1), T));
        }
        assert!(
            !i.deposit_lookup_allowed(ip(1), T + 30),
            "one client's share"
        );
        assert!(i.deposit_lookup_allowed(ip(2), T + 30), "another is served");
        assert!(
            i.deposit_lookup_allowed(ip(1), T + 60),
            "and the first again next minute"
        );
        let mut i = Inner::default();
        for n in 0..DEPOSIT_LOOKUPS_A_MINUTE {
            assert!(i.deposit_lookup_allowed(ip(n as u8), T));
        }
        assert!(
            !i.deposit_lookup_allowed(ip(200), T + 59),
            "the board's share"
        );
        assert!(!i.deposit_lookup_allowed(None, T + 59));
        assert!(i.deposit_lookup_allowed(ip(200), T + 60));
    }

    /// What wallets before 1.0.56 send is still read: a sell offer with the choice the
    /// board no longer keeps, and a buy offer without a deposit (refused by `list`).
    #[test]
    fn a_post_from_an_older_wallet_is_still_read() {
        let mut v = serde_json::to_value(offer("a", Side::SellGnr)).unwrap();
        v["prepaid"] = json!(false);
        v["reserve"] = json!("00");
        let p: OfferPost = serde_json::from_value(v).unwrap();
        assert!(p.reserve.is_some() && p.funding.is_none() && p.auto.is_none());
        let p: OfferPost =
            serde_json::from_value(serde_json::to_value(offer("b", Side::BuyGnr)).unwrap())
                .unwrap();
        assert!(p.funding.is_none());
        assert_eq!(
            Inner::default().list(p.offer, None, None, T),
            Err(board::FUNDING_MISSING)
        );
    }

    /// A board file from 1.0.55, when a buy offer could be listed without bitcoin. Its
    /// covered offers stay; the buy offer nothing stands behind goes; swaps in progress
    /// and mailboxes are not touched. This happens once.
    #[test]
    fn a_board_from_before_buy_offers_had_deposits_keeps_what_is_covered() {
        let dir = std::env::temp_dir().join(format!("gk-board3-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = now();
        let o = |id: &str, side: Side| {
            let mut o = offer(id, side);
            o.expires = t + 3600;
            o
        };
        let b = |n: &str| json!({"nullifiers": [n, format!("d{n}")], "anchor": "root"});
        let old = json!({
            "offers": [o("a", Side::SellGnr), o("b", Side::BuyGnr), o("c", Side::BuyGnr), o("s", Side::SellGnr)],
            "coin_offers": [],
            "pending": [[o("d", Side::BuyGnr), t], [o("p", Side::SellGnr), t]],
            "inbox": [["ff".repeat(32), [[t, "sealed"]]]],
            "rules": 2,
            "backing": [["a", b("n1")], ["s", b("n2")], ["p", b("n3")]],
            "deposits": [["b", "addr1"]],
            "prepaid": ["a"],
            "funded": ["d"],
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
        let (b, i) = open();
        let mut ids: Vec<_> = i.offers.keys().cloned().collect();
        ids.sort();
        assert_eq!(
            ids,
            ["a", "b", "s"],
            "the buy offer without bitcoin is gone"
        );
        assert!(!i.seen.contains_key("c") && !i.deposits.contains_key("c"));
        let mut running: Vec<_> = i.pending.keys().cloned().collect();
        running.sort();
        assert_eq!(running, ["d", "p"], "swaps in progress go on");
        assert!(i.funded.contains("d"));
        assert_eq!(i.messages, 1);
        assert_eq!(i.backing.len(), 3);
        assert_eq!(i.deposits.len(), 1);
        b.write_board_now(&i);
        let saved: Saved = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            (saved.rules, saved.offers.len(), saved.pending.len()),
            (RULES, 3, 2)
        );
        let (_, mut i) = open();
        assert_eq!(i.offers.len(), 3);
        // The deposit that was there is still spoken for, the dropped offer's id is free.
        assert_eq!(
            i.list(o("e", Side::BuyGnr), None, at("addr1"), t),
            Err(DEPOSIT_TAKEN)
        );
        assert_eq!(i.list(o("c", Side::BuyGnr), None, at("addr2"), t), Ok(()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_same_coins_back_one_thing_at_a_time() {
        let twice = Err("this GNR already backs another offer on the board");
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), backing("n1", "d1"), None, T),
            Ok(())
        );
        // The same note behind a second offer, as either of the proof's two inputs.
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n1", "d2"), None, T),
            twice
        );
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("d2", "n1"), None, T),
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
            i.list(offer("b", Side::SellGnr), backing("n1", "d2"), None, T),
            twice
        );
        // The same id cannot be used again either while the swap runs.
        assert!(
            i.list(offer("a", Side::SellGnr), backing("n9", "d9"), None, T)
                .is_err()
        );
        // The swap is over (its maker takes the entry down): the coins are free again.
        i.take_down("a");
        assert!(i.backing.is_empty() && i.pending.is_empty());
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n1", "d2"), None, T),
            Ok(())
        );
        // Other coins are another matter.
        assert_eq!(
            i.list(offer("c", Side::SellGnr), backing("n2", "d3"), None, T),
            Ok(())
        );
        assert_eq!(i.backing.len(), 2);
    }

    #[test]
    fn coins_are_free_again_when_their_entry_runs_out() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), backing("n1", "d1"), None, T),
            Ok(())
        );
        assert_eq!(
            i.list(offer("b", Side::SellGnr), backing("n2", "d2"), None, T),
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
            i.list(offer("c", Side::SellGnr), backing("n2", "d9"), None, T),
            Ok(())
        );
    }

    /// A board file that cannot be read is moved aside, never written over by the empty
    /// board that starts in its place.
    #[test]
    fn an_unreadable_board_file_is_kept_aside() {
        let dir =
            std::env::temp_dir().join(format!("gk-board-bad-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("swap-board.json");
        std::fs::write(&file, b"{ damaged").unwrap();
        let b = Board::default();
        b.set_path(dir.join("swap-trades.json"));
        let mut i = Inner::default();
        b.load(&mut i);
        assert!(i.offers.is_empty());
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("swap-board.unreadable-")
            })
            .collect();
        assert_eq!(kept.len(), 1, "moved aside");
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"{ damaged");
        b.write_board_now(&i);
        assert_eq!(
            std::fs::read(kept[0].path()).unwrap(),
            b"{ damaged",
            "not written over"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let made = |id: &str, side: Side| {
            let mut o = offer(id, side);
            o.expires = t + 3600;
            o
        };
        // What 1.0.39 wrote: no rules, no backing.
        let old = json!({
            "offers": [made("a", Side::SellGnr), made("b", Side::BuyGnr)],
            "coin_offers": [],
            "pending": [[made("c", Side::SellGnr), t], [made("d", Side::BuyGnr), t]],
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
            i.list(made("e", Side::SellGnr), backing("n1", "d1"), None, t),
            Ok(())
        );
        assert_eq!(
            i.list(made("f", Side::SellGnr), backing("n2", "d2"), None, t),
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
            i.list(made("g", Side::SellGnr), backing("n2", "d3"), None, t)
                .is_err()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn taking_an_offer_down_leaves_others_alone() {
        let mut i = Inner::default();
        assert_eq!(
            i.list(offer("a", Side::SellGnr), backing("n1", "d1"), None, T),
            Ok(())
        );
        assert_eq!(
            i.list(offer("b", Side::BuyGnr), None, at("addr1"), T),
            Ok(())
        );
        i.take_down("a");
        assert!(i.offers.contains_key("b") && i.seen.contains_key("b"));
        assert!(!i.offers.contains_key("a") && !i.seen.contains_key("a"));
    }
}
