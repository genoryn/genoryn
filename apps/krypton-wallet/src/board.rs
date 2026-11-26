//! The offer board from the wallet's side: publish and take offers, and let the wallets
//! pass the three swap codes through the board's sealed mailboxes by themselves. Also
//! reports completed swaps for the market price and caches that price.

use crate::{App, Multi, PUBLIC_NODE, S, blocking, check_open, err};
use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::Json;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use krypton_core::Network;
use krypton_core::emission::{format_amount, parse_amount};
use krypton_swap::bitcoin::{Amount, Denomination};
use krypton_swap::board::{self, MAX_OFFER_SATS, Note, Offer, Side, Trade, offer_key};
use krypton_swap::deposit;
use krypton_swap::engine::{self, Engine, Msg, Role, Stage};
use krypton_wallet_core::Wallet;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

mod quick;
use quick::{advance_quick, end_promises, list_rests, part_answers};
pub use quick::{btc_info, btc_send, quote as quick_quote, start as quick_start};

const STORE_KEY: &str = "board";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MyOffer {
    n: u32,
    offer: Offer,
    cancel_secret: String,
    btc_address: String,
    taken: bool,
    cancelled: bool,
    /// Put back on the board after its swap ended before anything was locked.
    #[serde(default)]
    republished: bool,
    /// Shown on the board as "in progress" while its swap runs (it counts for the price),
    /// and taken off again once the swap is over.
    #[serde(default)]
    pending_sent: bool,
    #[serde(default)]
    pending_cleared: bool,
    /// When the board was last told that the swap still runs.
    #[serde(default)]
    pending_at: u64,
    /// Sell offers from 1.0.40: the GNR set aside for the offer and proven to the board.
    #[serde(default)]
    reserve: Option<Reserve>,
    /// The board was told that the bitcoin of this offer's swap has been paid.
    #[serde(default)]
    funded_sent: bool,
    /// Buy offers from 1.0.41: the bitcoin paid in advance for the offer.
    #[serde(default)]
    fund: Option<Fund>,
    /// Sell offers from 1.0.43: GNR is locked only for a buyer whose bitcoin this wallet
    /// has seen at an address of the buyer's own wallet.
    #[serde(default)]
    prepaid_only: bool,
    /// From 1.0.44: the offer's price follows the market price.
    #[serde(default)]
    follow: Option<Follow>,
    /// The ids this offer was listed under before its price last moved (the newest
    /// fifty). Someone who asks for one of them is told that listing is gone.
    #[serde(default)]
    was: Vec<String>,
    /// From 1.0.46: a Quick Swap offer, sold (or bought) in parts by itself.
    #[serde(default)]
    auto: Option<AutoCfg>,
    /// The part of this offer its swap is for (GNR, sat), when it is not the whole offer.
    #[serde(default)]
    part: Option<(u64, u64)>,
    /// Buy offers: where the payment of a part leaves the rest of the bitcoin.
    #[serde(default)]
    rest_deposit: Option<u32>,
    /// The rest of the offer after its part was put on the board again (or let go).
    #[serde(default)]
    rest_done: bool,
    /// The board was asked to count the buyer's deposit for this offer's swap: the
    /// request's id and the secret that takes the promise back.
    #[serde(default)]
    promise: Option<(String, String)>,
    /// From 1.0.48: why the wallet took the offer off the board by itself (its trigger
    /// price was reached).
    #[serde(default)]
    stopped: Option<String>,
}

/// A Quick Swap offer's setting: the least GNR one part may be for, and the group of
/// offers it was published with (one Quick Swap offer is listed as a few lots, so that
/// several people can swap at once).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct AutoCfg {
    min: u64,
    group: String,
}

/// An offer that follows the market (1.0.44). What it stands for is fixed: the GNR of a
/// sell offer, the bitcoin of a buy offer. What is asked for it moves with the board's
/// market price: when that has moved by a hundredth or more, the wallet takes the offer
/// off the board and lists it again at the new price, at most every ten minutes and by
/// at most a twentieth at a time. Nothing is locked at a price: the price of a sale is
/// the offer's price at the moment a buyer's wallet asks for it, and a request with any
/// other amounts is refused.
///
/// The market price is what the board says it is. So an offer never follows it past its
/// owner's limit: a sell offer never asks less than `limit`, a buy offer never pays more.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Follow {
    /// Sat per GNR: the lowest price of a sell offer, the highest of a buy offer.
    limit: f64,
    /// When the price was last set, and how often it has been moved.
    #[serde(default)]
    at: u64,
    #[serde(default)]
    moves: u32,
    /// From 1.0.48: where the offer stands against the market price, as a fraction (0.02:
    /// two percent above it; -0.01: one percent below).
    #[serde(default)]
    margin: f64,
    /// From 1.0.48: the market price (sat per GNR) at which the offer is taken off the
    /// board by itself: a sell offer when the market falls to it, a buy offer when it
    /// rises to it.
    #[serde(default)]
    trigger: Option<f64>,
}

/// How far from the market price a following offer may stand (1.0.48): further out it
/// would be marked far from the market and Quick Swap would not take it.
const FOLLOW_MAX_MARGIN: f64 = 0.25;

/// Whether the market price `market` has reached the trigger of an offer of `side`.
fn triggered(side: Side, f: &Follow, market: f64) -> bool {
    f.trigger.is_some_and(|t| match side {
        Side::SellGnr => market <= t,
        Side::BuyGnr => market >= t,
    })
}

/// A following offer's price is looked at this often, moved only when the market is at
/// least `FOLLOW_MIN_MOVE` away from it, and then by at most `FOLLOW_MAX_STEP`: a price
/// that jumps (a thin market, a wrong report) is followed slowly, not at once.
const FOLLOW_EVERY_SECS: u64 = 10 * 60;
const FOLLOW_MIN_MOVE: f64 = 0.01;
const FOLLOW_MAX_STEP: f64 = 0.05;
/// A market price is followed if it rests on at least this many completed swaps. One is
/// enough: what protects a following offer from a thin or pushed price is its owner's
/// limit and the size of a step, not a count.
const FOLLOW_MIN_SWAPS: u64 = 1;

/// The board's market price in sat per GNR, if there is one to follow.
fn market_price(m: &Value) -> Option<f64> {
    let p = m["price"].as_f64()?;
    (m["priced"].as_u64().unwrap_or(0) >= FOLLOW_MIN_SWAPS && p.is_finite() && p > 0.0).then_some(p)
}

/// The price (sat per GNR) a following offer of `side` stands at, given the market: the
/// market price with the offer's margin, never past the owner's limit.
fn follow_price(side: Side, f: &Follow, market: f64) -> f64 {
    within_limit(side, f, market * (1.0 + f.margin))
}

fn within_limit(side: Side, f: &Follow, price: f64) -> f64 {
    match side {
        Side::SellGnr => price.max(f.limit),
        Side::BuyGnr => price.min(f.limit),
    }
}

/// The amounts of an offer of `side` at `price`: a sell offer keeps its GNR, a buy offer
/// its bitcoin.
fn amounts_at(side: Side, gnr: u64, sats: u64, price: f64) -> Option<(u64, u64)> {
    if !(price.is_finite() && price > 0.0) {
        return None;
    }
    let (g, s) = match side {
        Side::SellGnr => (gnr as f64, (gnr as f64 / 1e8 * price).round()),
        Side::BuyGnr => ((sats as f64 / price * 1e8).round(), sats as f64),
    };
    let ok = g > (2 * krypton_wallet_core::DEFAULT_FEE) as f64
        && g < 2.1e15
        && s >= 10_000.0
        && s <= board::MAX_OFFER_SATS as f64;
    ok.then_some((g as u64, s as u64))
}

/// The new amounts of the following offer `o`, if the market has moved enough for its
/// price to be moved: one step towards the market, never past the owner's limit.
fn follow_target(o: &Offer, f: &Follow, market: f64) -> Option<(u64, u64)> {
    let at = o.price();
    if !(at.is_finite() && at > 0.0) {
        return None;
    }
    let want = market * (1.0 + f.margin);
    let step = want.clamp(at * (1.0 - FOLLOW_MAX_STEP), at * (1.0 + FOLLOW_MAX_STEP));
    let to = within_limit(o.side, f, step);
    if (to - at).abs() / at < FOLLOW_MIN_MOVE {
        return None;
    }
    amounts_at(o.side, o.gnr, o.sats, to).filter(|x| *x != (o.gnr, o.sats))
}

/// Moves the prices of this wallet's following offers to the market. An offer is taken
/// off the board before it is listed again: its GNR (or its deposit) stands behind one
/// offer at a time. The offer keeps its GNR, its deposit, its key and its week.
fn follow_market(e: &mut Engine<'_, Multi>, base: &str, st: &mut State) {
    let Some(market) = market_price(&cached_market()["market"]) else {
        return;
    };
    let t = now();
    let every = if e.wallet.network() == Network::Devnet {
        20
    } else {
        FOLLOW_EVERY_SECS
    };
    for m in st.offers.iter_mut() {
        let Some(f) = m.follow.clone() else { continue };
        if m.taken || m.cancelled || m.offer.expires <= t + 900 {
            continue;
        }
        if triggered(m.offer.side, &f, market) {
            let why = format!(
                "Taken off the board by itself: the market price reached your trigger price of {} BTC per GNR.",
                btc_text(f.trigger.unwrap_or_default().round() as u64)
            );
            if take_down(e.wallet, base, m, &why).is_ok() {
                m.stopped = Some(why);
            }
            continue;
        }
        if f.at + every > t {
            continue;
        }
        // Only an offer that is on the board has a price to move.
        let listed = m
            .reserve
            .as_ref()
            .is_none_or(|r| r.posted && !r.released && r.cm.is_some())
            && m.fund
                .as_ref()
                .is_none_or(|x| x.posted && x.closed.is_none() && !x.handed);
        if !listed {
            continue;
        }
        let Some((gnr, sats)) = follow_target(&m.offer, &f, market) else {
            continue;
        };
        let secret: [u8; 32] = rand::random();
        let mut next = Offer {
            id: hex::encode(rand::random::<[u8; 16]>()),
            gnr,
            sats,
            cancel: board::cancel_hash(&secret),
            nonce: 0,
            ..m.offer.clone()
        };
        let down = json!({"id": m.offer.id, "secret": m.cancel_secret});
        if m.reserve.is_none() && m.fund.is_none() {
            // Nothing stands behind it but its maker's word: the new one first.
            next.seal_pow();
            if post(base, "/v1/swap/offers", &next).is_err() {
                continue;
            }
            let _ = post(base, "/v1/swap/offers/cancel", &down);
        } else if post(base, "/v1/swap/offers/cancel", &down).is_err() {
            continue;
        }
        let moving = "Following the market: publishing at the new price.";
        if let Some(r) = m.reserve.as_mut() {
            // The note is held for the new offer from this moment on.
            if let Some(cm) = r.note() {
                let _ = e.wallet.hold(cm, &next.id);
            }
            r.posted = false;
            r.last_try = 0;
            r.tries = 0;
            r.status = moving.into();
        }
        if let Some(x) = m.fund.as_mut() {
            x.posted = false;
            x.marked = false;
            x.last_check = 0;
            x.tries = 0;
            x.status = moving.into();
        }
        m.was.push(std::mem::replace(&mut m.offer, next).id);
        if m.was.len() > 50 {
            m.was.remove(0);
        }
        m.cancel_secret = hex::encode(secret);
        m.follow = Some(Follow {
            at: t,
            moves: f.moves + 1,
            ..f
        });
        advance(e, base, m);
        advance_buy(e, base, m);
    }
}

/// The bitcoin behind a buy offer. Publishing shows an address of this wallet's own
/// (`krypton_swap::deposit`). Once the bitcoin is there and confirmed the offer is listed;
/// a seller's take is paid from it without the buyer doing anything; and what is at the
/// address when the offer ends goes back to the buyer's own Bitcoin address.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Fund {
    /// The number of the deposit address, and the address.
    d: u32,
    address: String,
    /// What is asked for (sat): the offer's bitcoin and what is set aside for the fee of
    /// the payment into the contract.
    need: u64,
    #[serde(default)]
    since: u64,
    /// What has arrived (sat), and how much of it is confirmed.
    #[serde(default)]
    seen: u64,
    #[serde(default)]
    confirmed: u64,
    /// The board has the offer; and it found the bitcoin and marks the offer.
    #[serde(default)]
    posted: bool,
    #[serde(default)]
    marked: bool,
    #[serde(default)]
    status: String,
    #[serde(default)]
    last_check: u64,
    #[serde(default)]
    tries: u32,
    #[serde(default)]
    reposts: u32,
    /// When the offer ended: from then on what is at the address is sent back.
    #[serde(default)]
    closed: Option<u64>,
    /// The offer was published again and the new offer has this deposit now.
    #[serde(default)]
    handed: bool,
    /// The transactions that sent bitcoin back.
    #[serde(default)]
    returned: Vec<String>,
}

/// How a buy offer pays: by hand when a seller takes it, from bitcoin deposited now, or
/// from the deposit of the offer it replaces.
enum Pay {
    Later,
    Deposit,
    Reuse(Fund),
}

/// After a buy offer ends its deposit address is looked at this long, so a payment that
/// arrives late is still sent back.
const FUND_WATCH_SECS: u64 = 3 * 24 * 3600;

fn btc_text(sats: u64) -> String {
    Amount::from_sat(sats).to_string_in(Denomination::Bitcoin)
}

/// Deposit numbers come in lanes: number = lane + `DEPOSIT_LANES` × count. Each copy of a
/// wallet (the same phrase on another computer, a restored file) picks its lane at random
/// the first time, so two copies do not hand out the same address; and the numbers stay
/// few enough to be looked through when a wallet is restored from its phrase.
const DEPOSIT_LANES: u32 = 64;

/// The number of a fresh deposit address. An address that has been paid before belongs
/// to another copy of this wallet and is passed over.
fn next_deposit(e: &Engine<'_, Multi>, st: &mut State) -> u32 {
    let lane = *st
        .lane
        .get_or_insert_with(|| rand::random::<u32>() % DEPOSIT_LANES);
    loop {
        let d = lane + DEPOSIT_LANES * st.deposits;
        st.deposits += 1;
        let used = e
            .btc
            .outputs(&e.deposit_address(d))
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !used || st.deposits >= 100_000 {
            return d;
        }
    }
}

/// Deposit numbers that an offer, a purchase or a swap still needs.
fn deposits_in_use(w: &Wallet, st: &State) -> std::collections::HashSet<u32> {
    st.offers
        .iter()
        .filter_map(|m| m.fund.as_ref())
        .filter(|f| f.closed.is_none() && !f.handed)
        .chain(
            st.takes
                .iter()
                .filter_map(|x| x.fund.as_ref())
                .filter(|f| f.closed.is_none()),
        )
        .map(|f| f.d)
        .chain(
            engine::load(w)
                .iter()
                .filter(|s| !s.stage.finished())
                .flat_map(|s| [s.fund, s.change_to])
                .flatten(),
        )
        // Where the rest of a Quick Swap buy offer waits for its next listing.
        .chain(
            st.offers
                .iter()
                .filter(|m| !m.rest_done)
                .filter_map(|m| m.rest_deposit),
        )
        .chain(std::iter::once(deposit::BALANCE))
        .collect()
}

/// A number for a new offer or take. Its key and mailbox derive from it and the wallet's
/// seed. It is random, so the same wallet open on two computers never gives two offers
/// the same key (counted numbers, as before 1.0.41, would).
fn fresh_n(st: &State) -> u32 {
    loop {
        let n = rand::random::<u32>() % 0x7000_0000 + 0x8000_0000;
        let used = st.offers.iter().any(|o| o.n == n)
            || st.takes.iter().any(|x| x.n == n)
            || st.links.iter().any(|l| l.n == n);
        if !used {
            return n;
        }
    }
}

/// The GNR behind a sell offer. Publishing sets it aside in a note of its own (merging
/// notes first if it is spread over many), proves that note to the board and only then
/// lists the offer; a take locks exactly that note.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Reserve {
    /// Commitment (hex) of the note, once there is one.
    #[serde(default)]
    cm: Option<String>,
    /// Our transfer that makes the note (hex txid), while it waits for a block.
    #[serde(default)]
    txid: Option<String>,
    /// The board has the offer with its proof of funds.
    #[serde(default)]
    posted: bool,
    /// What is happening now, for the Swap tab.
    #[serde(default)]
    status: String,
    /// Rounds of merging notes so far.
    #[serde(default)]
    merges: u32,
    /// Times in a row the board refused the offer, and times it had to be posted again.
    #[serde(default)]
    tries: u32,
    #[serde(default)]
    reposts: u32,
    #[serde(default)]
    last_try: u64,
    /// The offer is over and its GNR was released: free at once, or from `free_at`
    /// (unix seconds) after a cancellation.
    #[serde(default)]
    released: bool,
    #[serde(default)]
    free_at: Option<u64>,
    /// When the board first listed the offer (unix seconds; 0: never, or before 1.0.42).
    #[serde(default)]
    listed: u64,
}

/// GNR of a cancelled sell offer can be sent again this long after the cancellation (or
/// when the offer would have ended, if that is sooner). It can go into a new offer at
/// once. This is a rule of the wallet, kept in its own file: the chain does not know it.
pub const CANCEL_HOLD_SECS: u64 = 24 * 3600;
/// From 1.0.42: GNR that was listed in a sell offer is committed for a week from the
/// moment the offer was published. Sold within the week, it goes to the buyer. If the
/// offer is cancelled, runs its time or is taken down before that, the GNR cannot be sent
/// until the week is over; it can go into a new offer at once (which starts a new week).
/// Like every hold, this is a rule of the wallet, kept in its own file.
pub const COMMIT_SECS: u64 = 7 * 24 * 3600;

/// When the GNR of a sell offer that is over may be sent again (`None`: now).
fn commit_until(r: &Reserve, expires: u64, t: u64) -> Option<u64> {
    let until = if r.listed > 0 {
        r.listed + COMMIT_SECS
    } else if r.posted {
        // Published before 1.0.42, under the rule of that time.
        (t + CANCEL_HOLD_SECS).min(expires)
    } else {
        return None;
    };
    (until > t).then_some(until)
}

/// What an ended offer's status says about its GNR.
fn commit_text(until: Option<u64>) -> &'static str {
    if until.is_some() {
        "Its GNR can go into a new offer now; it can be sent again 7 days after the offer was published."
    } else {
        "Its GNR is free again."
    }
}

/// A buy offer we took is answered by its maker's wallet within seconds. Without an
/// answer for this long, the take is let go of: nothing was locked or paid.
const TAKE_ANSWER_SECS: u64 = 10 * 60;
/// The fee of the transfers that set GNR aside (and merge notes for it).
const ASIDE_FEE: u64 = krypton_wallet_core::DEFAULT_FEE;

fn h32(s: &str) -> Option<krypton_core::Hash32> {
    hex::decode(s).ok()?.try_into().ok()
}

impl Reserve {
    fn note(&self) -> Option<krypton_core::Hash32> {
        self.cm.as_deref().and_then(h32)
    }
}

/// Frees GNR that is set aside for an offer that is over: one that ran its time, one
/// whose swap ended, or one this wallet has no record of (a record that was lost, or
/// never written, must not keep coins out of reach). Run on every background round,
/// whether or not there is anything else to do, and whether or not the board or a
/// Bitcoin server can be reached: no GNR stays set aside for nothing.
pub fn reconcile_holds(w: &mut Wallet) {
    let holds: Vec<_> = w
        .holds()
        .into_iter()
        .filter(|h| h.until.is_none())
        .collect();
    if holds.is_empty() {
        return;
    }
    let mut st = load(w);
    let swaps = engine::load(w);
    let t = now();
    let mut changed = false;
    for h in holds {
        let runs = st
            .links
            .iter()
            .filter(|l| l.offer == h.tag)
            .filter_map(|l| swaps.iter().find(|s| s.id == l.swap_id))
            .any(|s| !s.stage.finished());
        let mut m = st.offers.iter_mut().find(|m| m.offer.id == h.tag);
        let over = match held_for(m.as_deref(), runs, t) {
            Held::Keep => continue,
            Held::Free => false,
            Held::Over => true,
        };
        // What the board listed stays committed for its week, also when the record says
        // the offer is over but the note was never released (a release that failed).
        let mut until = None;
        if let Some(m) = m.as_mut() {
            let (expires, taken) = (m.offer.expires, m.taken);
            if let Some(r) = m.reserve.as_mut() {
                until = commit_until(r, expires, t);
                if over {
                    r.released = true;
                    r.free_at = until;
                    if !taken {
                        r.status = format!("The offer ran its time. {}", commit_text(until));
                    }
                    changed = true;
                }
            }
        }
        let _ = w.release(&h.cm, until);
    }
    if changed {
        let _ = save(w, &st);
    }
}

/// What becomes of GNR that is set aside for an offer.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    /// The offer is open, or its swap can still lock the coins.
    Keep,
    /// The wallet has no open offer for them (none on record, or one already ended).
    Free,
    /// The offer ended without a sale: it ran its time, or its swap ended before
    /// anything was locked. What was listed stays committed for its week.
    Over,
}

/// Whether the GNR set aside for the offer `m` (`None`: the wallet has no record of it)
/// must stay set aside at `t`. `swap_runs`: a swap of that offer is not finished.
fn held_for(m: Option<&MyOffer>, swap_runs: bool, t: u64) -> Held {
    match m {
        None => Held::Free,
        Some(m) if m.cancelled || m.reserve.as_ref().is_none_or(|r| r.released) => Held::Free,
        Some(m) if m.taken => {
            if swap_runs {
                Held::Keep
            } else {
                Held::Over
            }
        }
        Some(m) if m.offer.expires <= t => Held::Over,
        Some(_) => Held::Keep,
    }
}

/// Whether the note behind an offer is in the wallet, in a block and unspent.
fn reserve_ready(w: &Wallet, r: &Reserve) -> bool {
    r.note()
        .and_then(|cm| w.note(&cm).cloned())
        .is_some_and(|n| n.position.is_some() && n.pending_spend.is_none())
}

/// The board drops a swap in progress that its maker has not confirmed for half an hour.
const PENDING_REFRESH_SECS: u64 = 10 * 60;

/// Moves a taken offer to "in progress" on the board, or says that its swap still runs.
/// `Err` with "no such offer" means the board no longer has it.
fn announce_pending(base: &str, m: &MyOffer, funded: bool) -> Result<()> {
    let mut body =
        json!({"id": m.offer.id, "offer": m.offer, "secret": m.cancel_secret, "funded": funded});
    // A part counts for what it is (boards before 1.0.46 count the whole offer).
    if let Some((g, s)) = m.part {
        body["part"] = json!({"id": m.offer.id, "gnr": g, "sats": s});
    }
    post(base, "/v1/swap/progress", &body).map(|_| ())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MyTake {
    n: u32,
    offer: Offer,
    btc_address: String,
    swap_id: Option<String>,
    gone: bool,
    /// A purchase whose bitcoin is paid in advance (from 1.0.41): the request goes to the
    /// seller once the bitcoin is at this wallet's deposit address, and the wallet pays
    /// the contract from it by itself.
    #[serde(default)]
    fund: Option<Fund>,
    /// When it was made, when it ended without a swap, and why (unix seconds; 0: unknown).
    #[serde(default)]
    at: u64,
    #[serde(default)]
    ended: u64,
    #[serde(default)]
    why: String,
    /// From 1.0.46: the part of a Quick Swap offer this take is for (GNR, sat), and the
    /// Quick Swap it belongs to.
    #[serde(default)]
    part: Option<(u64, u64)>,
    #[serde(default)]
    quick: Option<String>,
    /// Selling: the board counts the buyer's deposit for this swap (request id, secret).
    #[serde(default)]
    promise: Option<(String, String)>,
}

impl MyTake {
    /// The amounts this take swaps: its part, or the whole offer.
    fn amounts(&self) -> (u64, u64) {
        self.part.unwrap_or((self.offer.gnr, self.offer.sats))
    }
}

/// A Quick Swap (1.0.46): buy or sell an amount of GNR at the best prices on the board,
/// across up to `board::MAX_PARTS` Quick Swap offers at once, each part its own swap.
/// A part that does not start (its offer was just taken by someone else, its maker went
/// offline) is replaced by another, never at a worse price than `limit`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct QuickOrder {
    id: String,
    buying: bool,
    want: board::Want,
    /// Sat per GNR: the most a part may cost (buying) or the least it may bring (selling).
    limit: f64,
    created: u64,
    /// The takes of its parts (their numbers), and every offer tried.
    takes: Vec<u32>,
    tried: Vec<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    last_plan: u64,
}

/// A Quick Swap looks for replacement parts this long; parts that started go on.
const QUICK_FILL_SECS: u64 = 10 * 60;
/// A part whose maker has not answered for this long is let go of and replaced.
const QUICK_ANSWER_SECS: u64 = 2 * 60;
/// A maker's wallet waits this long for the seller of a part to answer its request.
const PART_ANSWER_SECS: u64 = 3 * 60;
/// How far the price of a replacement part may be from the price shown (a hundredth).
pub const QUICK_SLIP: f64 = 0.01;

/// A purchase waits this long for its bitcoin before it is called off.
const TAKE_FUND_SECS: u64 = 2 * 3600;
/// A purchase whose bitcoin is there but which cannot start (the seller went offline,
/// the board cannot be reached) is called off after this long.
const TAKE_START_SECS: u64 = 6 * 3600;

/// A swap whose codes travel through the board: our key number, the other side's key.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Link {
    swap_id: String,
    offer: String,
    n: u32,
    peer: String,
    sent: Vec<String>,
    /// The other side was told this swap ended (cancelled or expired).
    #[serde(default)]
    gone_sent: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    next: u32,
    /// How many deposit addresses for buy offers this wallet has handed out.
    #[serde(default)]
    deposits: u32,
    /// The lane of this copy of the wallet (see [`next_deposit`]).
    #[serde(default)]
    lane: Option<u32>,
    offers: Vec<MyOffer>,
    takes: Vec<MyTake>,
    links: Vec<Link>,
    seen: Vec<String>,
    reported: Vec<String>,
    /// Reports the board has not taken yet: hash lock, last try, tries (1.0.44).
    #[serde(default)]
    report_tries: Vec<(String, u64, u32)>,
    /// This copy of the wallet (random; another computer with the same phrase has another).
    #[serde(default)]
    device: String,
    /// Swaps that other copies of this wallet reported, and what this copy reported last.
    #[serde(default)]
    mirror: Vec<Mirrored>,
    #[serde(default)]
    mirror_sent: Vec<(String, String)>,
    #[serde(default)]
    mirror_beat: u64,
    /// Quick Swaps (1.0.46), newest last.
    #[serde(default)]
    quick: Vec<QuickOrder>,
}

// One wallet on several computers.
//
// A swap is a record in the wallet file of the computer that started it. So that the same
// wallet on another computer can show the swap, and carry it on if the first computer is
// gone, every copy reports its swaps to a mailbox on the board that only copies of this
// wallet can find and read: its key derives from the wallet's seed, the reports are
// sealed to it, and each carries a tag made with the seed. The board sees ciphertext.

/// The number of the key of that mailbox (outside the numbers of offers and takes).
const MIRROR_N: u32 = 0xF000_0001;

/// A swap as another copy of this wallet last reported it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Mirrored {
    device: String,
    at: u64,
    swap: engine::Swap,
    #[serde(default)]
    link: Option<Link>,
}

#[derive(Serialize, Deserialize)]
struct MirrorMsg {
    device: String,
    at: u64,
    #[serde(default)]
    swap: Option<engine::Swap>,
    #[serde(default)]
    link: Option<Link>,
    /// Made with the wallet's seed over everything above: only a copy of this wallet can
    /// have written the report.
    tag: String,
}

fn mirror_tag(w: &Wallet, m: &MirrorMsg) -> String {
    let body = serde_json::to_vec(&(&m.device, m.at, &m.swap, &m.link)).unwrap_or_default();
    hex::encode(krypton_crypto::hash::tagged_parts(
        "Krypton/board/mirror/v1",
        &[w.swap_seed().as_slice(), &body],
    ))
}

/// What of a swap another copy needs to hear about again.
fn mirror_digest(s: &engine::Swap) -> String {
    format!(
        "{:?}|{}|{}|{}|{}",
        s.stage,
        s.deposit.is_some(),
        s.lock_cm.is_some(),
        s.btc_spend.is_some(),
        s.paid_tx.is_some()
    )
}

fn mirror_post(e: &Engine<'_, Multi>, base: &str, mut m: MirrorMsg) -> Result<()> {
    let key = key_of(e.wallet, MIRROR_N);
    m.tag = mirror_tag(e.wallet, &m);
    let env = board::seal(key.public(), &key, &serde_json::to_vec(&m)?)?;
    post(base, "/v1/swap/inbox", &env)?;
    Ok(())
}

/// Reports this copy's swaps whose state changed (a few per round: each report costs a
/// second of proof of work), and once an hour that it is still running them.
fn mirror_publish(e: &Engine<'_, Multi>, base: &str, st: &mut State, swaps: &[engine::Swap]) {
    let t = now();
    if st.device.is_empty() {
        st.device = hex::encode(rand::random::<[u8; 8]>());
    }
    let mut sent = 0;
    for s in swaps {
        let d = mirror_digest(s);
        let known = st.mirror_sent.iter().position(|(id, _)| *id == s.id);
        // Unchanged, or over before this copy ever reported it (old history).
        if known.is_some_and(|i| st.mirror_sent[i].1 == d)
            || (known.is_none() && s.stage.finished())
        {
            continue;
        }
        if sent >= 3 {
            break;
        }
        let mut copy = s.clone();
        copy.log.clear();
        let msg = MirrorMsg {
            device: st.device.clone(),
            at: t,
            swap: Some(copy),
            link: st.links.iter().find(|l| l.swap_id == s.id).cloned(),
            tag: String::new(),
        };
        if mirror_post(e, base, msg).is_err() {
            break;
        }
        match known {
            Some(i) => st.mirror_sent[i].1 = d,
            None => st.mirror_sent.push((s.id.clone(), d)),
        }
        sent += 1;
    }
    if swaps.iter().any(|s| !s.stage.finished()) && st.mirror_beat + 3600 < t {
        let beat = MirrorMsg {
            device: st.device.clone(),
            at: t,
            swap: None,
            link: None,
            tag: String::new(),
        };
        if mirror_post(e, base, beat).is_ok() {
            st.mirror_beat = t;
        }
    }
    let n = st.mirror_sent.len();
    if n > 300 {
        st.mirror_sent.drain(..n - 300);
    }
}

/// Reads what other copies of this wallet reported. Run on every background round; it
/// asks the board at most every two minutes.
pub fn mirror_read(a: &App, w: &mut Wallet) {
    static LAST: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    let who = w.address().short();
    {
        let mut last = LAST.get_or_init(Default::default).lock();
        if last
            .get(&who)
            .is_some_and(|i| i.elapsed() < Duration::from_secs(120))
        {
            return;
        }
        last.insert(who, Instant::now());
    }
    let key = key_of(w, MIRROR_N);
    let inbox = board::inbox_of(key.public());
    let Ok(v) = get(&board_url(a), &format!("/v1/swap/inbox/{inbox}")) else {
        return;
    };
    let mut st = load(w);
    let t = now();
    let mut changed = false;
    if st.device.is_empty() {
        st.device = hex::encode(rand::random::<[u8; 8]>());
        changed = true;
    }
    for m in v["messages"].as_array().cloned().unwrap_or_default() {
        let Some(data) = m["data"].as_str() else {
            continue;
        };
        let id = hash(data);
        if st.seen.contains(&id) {
            continue;
        }
        st.seen.push(id);
        changed = true;
        let Ok((_, body)) = board::open(&key, data) else {
            continue;
        };
        let Ok(msg) = serde_json::from_slice::<MirrorMsg>(&body) else {
            continue;
        };
        // Only a copy of this wallet can make the tag; a report of our own is skipped,
        // and so is one from the future (a wrong clock must not outrank real reports).
        if msg.tag != mirror_tag(w, &msg) || msg.device == st.device || msg.at > t + 600 {
            continue;
        }
        let Some(s) = msg.swap else { continue };
        let new = Mirrored {
            device: msg.device,
            at: msg.at,
            swap: s,
            link: msg.link,
        };
        match st.mirror.iter_mut().find(|x| x.swap.id == new.swap.id) {
            Some(x) if x.at <= new.at => *x = new,
            Some(_) => {}
            None => st.mirror.push(new),
        }
    }
    // What this copy runs itself is not another copy's; and old reports go.
    let local: std::collections::HashSet<String> =
        engine::load(w).into_iter().map(|s| s.id).collect();
    let before = st.mirror.len();
    st.mirror
        .retain(|x| !local.contains(&x.swap.id) && x.at + 7 * 24 * 3600 > t);
    let n = st.mirror.len();
    if n > 50 {
        st.mirror.drain(..n - 50);
    }
    if changed || st.mirror.len() != before {
        let _ = save(w, &st);
    }
}

/// Swaps of this wallet that run on another computer, for the Swap tab, and the numbers
/// of those in which this wallet sells (their locks are not shown a second time).
pub fn mirrored(w: &Wallet) -> (Vec<Value>, Vec<u32>) {
    let st = load(w);
    let t = now();
    let local: std::collections::HashSet<String> =
        engine::load(w).into_iter().map(|s| s.id).collect();
    let list: Vec<&Mirrored> = st
        .mirror
        .iter()
        .filter(|m| !local.contains(&m.swap.id))
        .filter(|m| !m.swap.stage.finished() || m.at + 24 * 3600 > t)
        .collect();
    let sells = list
        .iter()
        .filter(|m| m.swap.role == Role::GnrSeller)
        .map(|m| m.swap.index)
        .collect();
    let v = list
        .iter()
        .rev()
        .take(20)
        .map(|m| {
            let s = &m.swap;
            json!({
                "id": s.id, "role": s.role, "stage": s.stage, "status": s.status,
                "finished": s.stage.finished(),
                "gnr_text": format_amount(s.gnr), "btc_text": btc_text(s.sats),
                "age": t.saturating_sub(m.at),
                "device": m.device.chars().take(6).collect::<String>(),
            })
        })
        .collect();
    (v, sells)
}

/// Carries a swap on here that another copy of this wallet started: its record, as that
/// copy last reported it, becomes a swap of this copy. Every step of a swap spends
/// something that can be spent once (the note behind the offer, the lock, the bitcoin
/// deposit), so if the other copy is running after all, the two cannot pay or lock twice.
fn adopt_swap(w: &mut Wallet, id: &str) -> Result<()> {
    let mut st = load(w);
    let i = st
        .mirror
        .iter()
        .position(|x| x.swap.id == id)
        .context("this swap is not known from another computer")?;
    ensure!(!st.mirror[i].swap.stage.finished(), "this swap is over");
    let mut swaps = engine::load(w);
    ensure!(
        !swaps
            .iter()
            .any(|s| s.id == id || s.index == st.mirror[i].swap.index),
        "this wallet already has this swap"
    );
    let m = st.mirror.remove(i);
    let mut s = m.swap;
    s.log
        .push((now(), "Carried on at this computer.".to_string()));
    swaps.push(s);
    engine::save(w, &swaps)?;
    if let Some(l) = m.link
        && !st.links.iter().any(|x| x.swap_id == l.swap_id)
    {
        st.links.push(l);
    }
    save(w, &st)
}

#[derive(Deserialize)]
pub struct AdoptReq {
    id: String,
    #[serde(default)]
    password: String,
}

pub async fn adopt(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<AdoptReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        let mut g = a.wallet.lock();
        let w = g.as_mut().context("wallet locked")?;
        if let Some(why) = crate::swaps::inactive(&a, Some(w)) {
            bail!(why);
        }
        adopt_swap(w, &r.id)
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

fn load(w: &Wallet) -> State {
    w.app_data(STORE_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn save(w: &mut Wallet, s: &State) -> Result<()> {
    w.set_app_data(STORE_KEY, serde_json::to_value(s)?)
}

fn now() -> u64 {
    krypton_core::now()
}

/// An offer or swap id as every wallet makes it: 32 hexadecimal digits.
fn swap_id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}

/// The board every wallet of a network meets on: genoryn.io on mainnet, otherwise the
/// first configured node.
pub fn board_url(a: &App) -> String {
    if a.network == Network::Mainnet {
        return PUBLIC_NODE.to_string();
    }
    let s = a.settings.lock().clone();
    if s.mode == "full" {
        a.local_node()
    } else {
        s.nodes
            .first()
            .cloned()
            .unwrap_or_else(|| PUBLIC_NODE.to_string())
    }
}

fn http() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("genoryn-krypton-wallet")
            .build()
            .expect("http client")
    })
}

pub(crate) fn get_json(base: &str, path: &str) -> Result<Value> {
    get(base, path)
}

pub(crate) fn post_json(base: &str, path: &str, body: &impl Serialize) -> Result<Value> {
    post(base, path, body)
}

fn get(base: &str, path: &str) -> Result<Value> {
    let r = http()
        .get(format!("{}{path}", base.trim_end_matches('/')))
        .send()?;
    let st = r.status();
    let v: Value = r.json().unwrap_or_default();
    ensure!(
        st.is_success(),
        "board: {}",
        v["error"].as_str().unwrap_or(st.as_str())
    );
    Ok(v)
}

fn post(base: &str, path: &str, body: &impl Serialize) -> Result<Value> {
    let r = http()
        .post(format!("{}{path}", base.trim_end_matches('/')))
        .json(body)
        .send()?;
    let st = r.status();
    let v: Value = r.json().unwrap_or_default();
    ensure!(
        st.is_success(),
        "board: {}",
        v["error"].as_str().unwrap_or(st.as_str())
    );
    Ok(v)
}

fn send_note(
    base: &str,
    to_key_hex: &str,
    from: &krypton_crypto::kem::SecretKey,
    note: &Note,
) -> Result<()> {
    let to = krypton_crypto::kem::PublicKey::from_bytes(&hex::decode(to_key_hex)?)?;
    let env = board::seal(&to, from, &serde_json::to_vec(note)?)?;
    post(base, "/v1/swap/inbox", &env)?;
    Ok(())
}

/// Market price and BTC/USD, refreshed by the background task.
fn market_cache() -> &'static Mutex<(Option<Instant>, Value)> {
    static M: OnceLock<Mutex<(Option<Instant>, Value)>> = OnceLock::new();
    M.get_or_init(|| Mutex::new((None, Value::Null)))
}

pub fn cached_market() -> Value {
    market_cache().lock().1.clone()
}

/// Refreshes the cached market (at most once a minute).
pub fn refresh_market(a: &App) {
    if market_cache()
        .lock()
        .0
        .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
    {
        return;
    }
    let base = board_url(a);
    let mut v = get(&base, "/v1/swap/market").unwrap_or(Value::Null);
    // BTC/USD through genoryn.io (it fetches mempool.space prices, so this wallet's address
    // is not shown to a third party); mempool.space directly if that fails.
    let price = |u: &str| {
        http()
            .get(u)
            .send()
            .and_then(|r| r.json::<Value>())
            .ok()
            .filter(|p| p["USD"].is_number())
    };
    if let Some(p) = price(&format!("{PUBLIC_NODE}/v1/btc-price"))
        .or_else(|| price("https://mempool.space/api/v1/prices"))
    {
        v["btc_usd"] = p["USD"].clone();
    }
    *market_cache().lock() = (Some(Instant::now()), v);
}

fn engine_for<'a>(
    a: &App,
    w: &'a mut Wallet,
    src: &'a Multi,
    btc: &'a dyn krypton_swap::btc::BtcChain,
    blocks: &'a dyn Fn(u64) -> Result<Vec<krypton_core::types::Transaction>>,
) -> Engine<'a, Multi> {
    let net = crate::swaps::btc_network(a);
    Engine {
        wallet: w,
        gnr: src,
        btc,
        btc_network: net,
        timing: krypton_swap::Timing::for_network(net),
        block_txs: blocks,
    }
}

/// Runs `f` with the active wallet's swap engine. Publishing an offer works before swaps
/// activate (offers already make a price); anything that starts a swap needs `active`.
fn with_engine<T>(
    a: &App,
    active: bool,
    f: impl FnOnce(&mut Engine<'_, Multi>, &str) -> Result<T>,
) -> Result<T> {
    let src = Multi(a.sources()?);
    let backend = krypton_swap::btc::connect(&crate::swaps::backend_spec(a))?;
    let mut guard = a.wallet.lock();
    let w = guard.as_mut().context("wallet locked")?;
    if active && let Some(why) = crate::swaps::inactive(a, Some(w)) {
        bail!(why);
    }
    let blocks = |h: u64| -> Result<Vec<krypton_core::types::Transaction>> {
        Ok(src.first()?.raw_block(h)?.txs)
    };
    let mut e = engine_for(a, w, &src, backend.as_ref(), &blocks);
    f(&mut e, &board_url(a))
}

#[allow(clippy::too_many_arguments)]
fn publish_offer(
    e: &mut Engine<'_, Multi>,
    base: &str,
    side: Side,
    gnr: u64,
    sats: u64,
    btc_address: &str,
    secs: u64,
    pay: Pay,
    prepaid_only: bool,
    follow: Option<Follow>,
    auto: Option<AutoCfg>,
) -> Result<Offer> {
    engine::btc_address(btc_address, e.btc_network)?;
    let secs = secs.clamp(15 * 60, board::MAX_OFFER_SECS);
    ensure!(sats >= 10_000, "offer at least 10,000 sat");
    ensure!(
        gnr > 2 * krypton_wallet_core::DEFAULT_FEE,
        "the GNR amount is too small"
    );
    let mut st = load(e.wallet);
    let n = fresh_n(&st);
    let key = key_of(e.wallet, n);
    let secret: [u8; 32] = rand::random();
    let mut offer = Offer {
        id: hex::encode(rand::random::<[u8; 16]>()),
        network: e.wallet.network().as_str().into(),
        btc_network: e.btc_network.to_core_arg().into(),
        side,
        gnr,
        sats,
        key: hex::encode(key.public().to_bytes()),
        expires: now() + secs,
        cancel: board::cancel_hash(&secret),
        nonce: 0,
    };
    let mut mine = MyOffer {
        n,
        offer: offer.clone(),
        cancel_secret: hex::encode(secret),
        btc_address: btc_address.trim().into(),
        taken: false,
        cancelled: false,
        republished: false,
        pending_sent: false,
        pending_cleared: false,
        pending_at: 0,
        reserve: None,
        funded_sent: false,
        fund: None,
        // A Quick Swap sell offer locks GNR only for buyers whose bitcoin is there.
        prepaid_only: (prepaid_only || auto.is_some()) && side == Side::SellGnr,
        follow,
        was: Vec::new(),
        auto: auto.clone(),
        part: None,
        rest_deposit: None,
        rest_done: false,
        promise: None,
        stopped: None,
    };
    if let Some(a) = &auto {
        board::auto_ok(&offer, &board::Auto { min: a.min })?;
    }
    let fund = match pay {
        _ if side == Side::SellGnr => None,
        Pay::Later => None,
        Pay::Reuse(f) => Some(f),
        Pay::Deposit => {
            // What is set aside for the fee of the payment follows today's fees.
            let rate = e
                .btc
                .fee_rate()
                .context("the Bitcoin server did not answer; try again in a moment")?;
            let need = sats
                .checked_add(deposit::fee_reserve(rate))
                .context("amount overflow")?;
            let d = next_deposit(e, &mut st);
            let address = e.deposit_address(d).to_string();
            Some(Fund {
                d,
                status: format!(
                    "Waiting for your bitcoin: send {} BTC to {address}. The offer is published by itself once it has one confirmation.",
                    btc_text(need)
                ),
                address,
                need,
                since: now(),
                ..Default::default()
            })
        }
    };
    if side == Side::SellGnr {
        // The GNR is set aside before anything is listed: an offer the wallet cannot
        // back never reaches the board, and the board checks the proof for itself.
        let _ = e.wallet.sync(e.gnr, |_, _| {});
        let mut r = Reserve::default();
        aside(e, &mine.offer, &mut r)?;
        mine.reserve = Some(r);
        // Listed right away when a note of the right amount was already there.
        advance(e, base, &mut mine);
        offer = mine.offer.clone();
    } else if fund.is_some() {
        // Listed once the bitcoin is at the deposit address: at once if it already is.
        mine.fund = fund;
        advance_buy(e, base, &mut mine);
        offer = mine.offer.clone();
    } else {
        ensure!(
            auto.is_none(),
            "a Quick Swap buy offer needs its bitcoin first"
        );
        offer.seal_pow();
        post(base, "/v1/swap/offers", &offer)?;
        mine.offer = offer.clone();
    }
    st.offers.push(mine);
    save(e.wallet, &st)?;
    Ok(offer)
}

/// What goes to the board with a Quick Swap offer.
fn auto_json(a: Option<&AutoCfg>) -> Option<Value> {
    a.map(|a| json!({ "min": a.min }))
}

/// A Quick Swap offer is listed as this many lots at most, so that several people can
/// swap with it at once (one swap runs per lot).
const MAX_LOTS: usize = 3;

/// How many lots a Quick Swap offer of `sats` bitcoin is listed as.
fn lots_for(sats: u64) -> usize {
    match sats {
        0..200_000 => 1,
        200_000..2_000_000 => 2,
        _ => MAX_LOTS,
    }
}

/// Splits (GNR, sat) into `k` lots at the same price: the first lot carries what does not
/// divide evenly. Bitcoin is rounded in the maker's favour (`board::part_sats`).
fn split_lots(side: Side, gnr: u64, sats: u64, k: usize) -> Vec<(u64, u64)> {
    let k = k.max(1) as u64;
    let whole = Offer {
        id: String::new(),
        network: String::new(),
        btc_network: String::new(),
        side,
        gnr,
        sats,
        key: String::new(),
        expires: 0,
        cancel: String::new(),
        nonce: 0,
    };
    let each = gnr / k;
    let mut v: Vec<(u64, u64)> = (0..k)
        .map(|i| {
            let g = if i == 0 { gnr - each * (k - 1) } else { each };
            (g, board::part_sats(&whole, g).unwrap_or(0))
        })
        .collect();
    // Buying: the lots together never pay more than the whole offer.
    if side == Side::BuyGnr {
        let total: u64 = v.iter().map(|x| x.1).sum();
        if total > sats {
            v[0].1 -= total - sats;
        }
    }
    v
}

/// A lot of a Quick Swap offer, not published yet: a sell lot gathers its GNR in the
/// background rounds (one lot after another, as the change of the one before arrives),
/// a buy lot waits for the bitcoin sent to its deposit.
#[allow(clippy::too_many_arguments)]
fn new_lot(
    e: &Engine<'_, Multi>,
    st: &State,
    side: Side,
    gnr: u64,
    sats: u64,
    btc_address: &str,
    expires: u64,
    follow: Option<Follow>,
    auto: AutoCfg,
) -> MyOffer {
    let n = fresh_n(st);
    let key = key_of(e.wallet, n);
    let secret: [u8; 32] = rand::random();
    let offer = Offer {
        id: hex::encode(rand::random::<[u8; 16]>()),
        network: e.wallet.network().as_str().into(),
        btc_network: e.btc_network.to_core_arg().into(),
        side,
        gnr,
        sats,
        key: hex::encode(key.public().to_bytes()),
        expires,
        cancel: board::cancel_hash(&secret),
        nonce: 0,
    };
    MyOffer {
        n,
        offer,
        cancel_secret: hex::encode(secret),
        btc_address: btc_address.trim().into(),
        taken: false,
        cancelled: false,
        republished: false,
        pending_sent: false,
        pending_cleared: false,
        pending_at: 0,
        reserve: (side == Side::SellGnr).then(|| Reserve {
            status: "Waiting to set this lot's GNR aside: the lot before it goes first (about a minute each).".into(),
            ..Default::default()
        }),
        funded_sent: false,
        fund: None,
        prepaid_only: side == Side::SellGnr,
        follow,
        was: Vec::new(),
        auto: Some(auto),
        part: None,
        rest_deposit: None,
        rest_done: false,
        promise: None,
        stopped: None,
    }
}

/// Publishes a Quick Swap offer as its lots. A sell offer sets the GNR of its first lot
/// aside at once (an error means the wallet cannot back it) and the others in the next
/// rounds. A buy offer pays its lots' deposits from the Bitcoin balance in one
/// transaction; they are listed once that has one confirmation.
#[allow(clippy::too_many_arguments)]
fn publish_lots(
    e: &mut Engine<'_, Multi>,
    base: &str,
    side: Side,
    gnr: u64,
    sats: u64,
    btc_address: &str,
    secs: u64,
    follow: Option<Follow>,
    min: Option<u64>,
) -> Result<Vec<String>> {
    engine::btc_address(btc_address, e.btc_network)?;
    let secs = secs.clamp(15 * 60, board::MAX_OFFER_SECS);
    let lots = split_lots(side, gnr, sats, lots_for(sats));
    ensure!(
        lots.iter()
            .all(|(g, s)| *s >= 10_000 && *g > 2 * krypton_wallet_core::DEFAULT_FEE),
        "the offer is too small: each lot needs at least 10,000 sat"
    );
    // The smallest part: what the user said, else what 10,000 sat buy at this price.
    let (g0, s0) = lots[lots.len() - 1];
    let floor = ((board::MIN_PART_SATS as u128 * g0 as u128).div_ceil(s0.max(1) as u128)) as u64;
    let min = min
        .unwrap_or(floor)
        .max(floor)
        .max(board::MIN_PART_GNR)
        .min(g0);
    let group = hex::encode(rand::random::<[u8; 8]>());
    let auto = AutoCfg { min, group };
    let expires = now() + secs;
    match side {
        Side::SellGnr => {
            let need: u64 = lots
                .iter()
                .map(|(g, _)| g + board::RESERVE_FEE + ASIDE_FEE)
                .sum();
            let have = available(e.wallet);
            ensure!(
                have >= need,
                "this wallet has {} GNR free; the offer needs {} (its amount and the fees). GNR in your other open offers, in swaps or still maturing does not count",
                format_amount(have),
                format_amount(need)
            );
            let (g, s) = lots[0];
            let first = publish_offer(
                e,
                base,
                side,
                g,
                s,
                btc_address,
                secs,
                Pay::Later,
                true,
                follow.clone(),
                Some(auto.clone()),
            )?;
            let mut ids = vec![first.id];
            let mut st = load(e.wallet);
            for (g, s) in lots.into_iter().skip(1) {
                let mut m = new_lot(
                    e,
                    &st,
                    side,
                    g,
                    s,
                    btc_address,
                    expires,
                    follow.clone(),
                    auto.clone(),
                );
                // Every lot of the offer ends with the first.
                m.offer.expires = st
                    .offers
                    .iter()
                    .find(|o| o.offer.id == ids[0])
                    .map(|o| o.offer.expires)
                    .unwrap_or(expires);
                ids.push(m.offer.id.clone());
                st.offers.push(m);
            }
            save(e.wallet, &st)?;
            Ok(ids)
        }
        Side::BuyGnr => {
            let rate = e
                .btc
                .fee_rate()
                .context("the Bitcoin server did not answer; try again in a moment")?;
            let reserve = deposit::fee_reserve(rate);
            let mut st = load(e.wallet);
            let mut made = Vec::new();
            for (g, s) in &lots {
                let mut m = new_lot(
                    e,
                    &st,
                    side,
                    *g,
                    *s,
                    btc_address,
                    expires,
                    follow.clone(),
                    auto.clone(),
                );
                let d = next_deposit(e, &mut st);
                m.fund = Some(Fund {
                    d,
                    address: e.deposit_address(d).to_string(),
                    need: s + reserve,
                    since: now(),
                    status: "Paid from your Bitcoin balance; the lot is published once that has one confirmation (about ten minutes).".into(),
                    ..Default::default()
                });
                st.offers.push(m.clone());
                made.push(m);
            }
            let to: Vec<(krypton_swap::bitcoin::Address, u64)> = made
                .iter()
                .filter_map(|m| m.fund.as_ref())
                .map(|f| (e.deposit_address(f.d), f.need))
                .collect();
            let total: u64 = to.iter().map(|x| x.1).sum();
            let b = quick::balance(e, &st)?;
            let free = b.confirmed.saturating_sub(b.committed);
            ensure!(
                free >= total + reserve,
                "your Bitcoin balance has {} BTC free and confirmed; this offer needs {} BTC and the network fee. Add bitcoin to your balance first",
                btc_text(free),
                btc_text(total)
            );
            // Saved before the payment: a payment whose record was lost would leave
            // bitcoin at deposits nothing watches.
            save(e.wallet, &st)?;
            if let Err(x) = e.pay_from(deposit::BALANCE, &to, deposit::BALANCE) {
                let mut st = load(e.wallet);
                st.offers
                    .retain(|o| !made.iter().any(|m| m.offer.id == o.offer.id));
                save(e.wallet, &st)?;
                return Err(x);
            }
            Ok(made.into_iter().map(|m| m.offer.id).collect())
        }
    }
}

/// One step of setting the GNR of a sell offer aside: finds or makes the note, or, when
/// the amount is spread over more than two notes, merges notes first (the next round
/// carries on). An error means the wallet cannot back the offer.
fn aside(e: &mut Engine<'_, Multi>, offer: &Offer, r: &mut Reserve) -> Result<()> {
    use krypton_wallet_core::Aside;
    let need = offer
        .gnr
        .checked_add(board::RESERVE_FEE)
        .context("amount overflow")?;
    match e.wallet.set_aside(e.gnr, need, ASIDE_FEE, &offer.id) {
        Ok(Aside::Ready(cm)) => {
            r.cm = Some(hex::encode(cm));
            r.txid = None;
            r.status = "Proving your GNR to the board.".into();
        }
        Ok(Aside::Pending { cm, txid }) => {
            r.cm = Some(hex::encode(cm));
            r.txid = Some(hex::encode(txid));
            r.status = "Setting your GNR aside (one transfer to yourself). The offer is published by itself after the next block.".into();
        }
        Err(x) if x.to_string().contains("NEEDS_CONSOLIDATION") => {
            ensure!(
                r.merges < 16,
                "your GNR is spread over too many small notes to gather for this offer"
            );
            let ids = e.wallet.consolidate(e.gnr, ASIDE_FEE, 64)?;
            r.merges += 1;
            r.status = format!(
                "Your GNR is in many small notes: merging them first (step {}, {} transactions). The offer is published by itself when that is done.",
                r.merges,
                ids.len()
            );
        }
        Err(x) => {
            let have = e.wallet.balance().spendable;
            if x.to_string().contains("insufficient") {
                bail!(
                    "this wallet has {} GNR free; the offer needs {} (its amount and the fees). GNR in your other open offers, in swaps or still maturing does not count",
                    format_amount(have),
                    format_amount(need + ASIDE_FEE)
                );
            }
            return Err(x);
        }
    }
    Ok(())
}

/// Ends an offer that could not be listed, or that the board took down. GNR that was
/// never listed is free at once; GNR that was stays committed for its week.
fn give_up(w: &mut Wallet, m: &mut MyOffer, why: String) {
    m.cancelled = true;
    let expires = m.offer.expires;
    if let Some(r) = m.reserve.as_mut() {
        let until = commit_until(r, expires, now());
        if let Some(cm) = r.note() {
            let _ = w.release(&cm, until);
        }
        r.released = true;
        r.free_at = until;
        r.status = why.replace("Its GNR is free again.", commit_text(until));
    }
}

/// Carries a sell offer one step further towards the board: gathers its GNR, waits for
/// the note to be in a block, then proves it and posts the offer.
fn advance(e: &mut Engine<'_, Multi>, base: &str, m: &mut MyOffer) {
    let Some(mut r) = m.reserve.clone() else {
        return;
    };
    if r.posted || m.cancelled || m.taken {
        return;
    }
    let t = now();
    if m.offer.expires <= t + 60 {
        give_up(
            e.wallet,
            m,
            "The offer ran out before it could be published. Its GNR is free again.".into(),
        );
        return;
    }
    let step = |e: &mut Engine<'_, Multi>, m: &mut MyOffer, r: &mut Reserve| -> Result<bool> {
        // No note yet: notes are being merged. Wait for those transfers, then try again.
        let Some(cm) = r.note() else {
            if e.wallet.balance().pending == 0 {
                aside(e, &m.offer, r)?;
            }
            return Ok(false);
        };
        if !reserve_ready(e.wallet, r) {
            // Still on its way into a block, unless its transfer expired unmined (the
            // wallet then forgets it): start again.
            let known = e.wallet.note(&cm).is_some()
                || r.txid
                    .as_deref()
                    .is_some_and(|id| e.wallet.history().iter().any(|h| h.txid == id));
            if !known {
                let _ = e.wallet.release(&cm, None);
                r.cm = None;
                r.txid = None;
                r.status =
                    "The transfer that set your GNR aside did not confirm; trying again.".into();
            }
            return Ok(false);
        }
        if r.last_try + 20 > t {
            return Ok(false);
        }
        r.last_try = t;
        // An offer on the board always has its note held, whatever became of the hold
        // meanwhile (an offer posted again after its cancellation was interrupted).
        if !e
            .wallet
            .holds()
            .iter()
            .any(|h| h.cm == cm && h.until.is_none() && h.tag == m.offer.id)
        {
            e.wallet.hold(cm, &m.offer.id)?;
        }
        let (owner, rcm) = board::reserve_output(&m.offer);
        let tx = e.wallet.prove_reserve(
            e.gnr,
            &cm,
            owner,
            m.offer.gnr,
            rcm,
            board::RESERVE_FEE,
            board::RESERVE_EXPIRY,
        )?;
        if !m.offer.pow_ok_at(now()) {
            m.offer.seal_pow();
        }
        let mut body = serde_json::to_value(&m.offer)?;
        body["reserve"] = json!(hex::encode(tx.to_bytes()));
        if m.prepaid_only {
            body["prepaid"] = json!(true);
        }
        if let Some(a) = auto_json(m.auto.as_ref()) {
            body["auto"] = a;
        }
        post(base, "/v1/swap/offers", &body)?;
        Ok(true)
    };
    match step(e, m, &mut r) {
        Ok(true) => {
            r.posted = true;
            if r.listed == 0 {
                r.listed = now();
            }
            r.tries = 0;
            r.status.clear();
        }
        Ok(false) => {}
        Err(x) => {
            let msg = x.to_string();
            if msg.contains("an offer with this id exists") {
                // It is on the board already.
                r.posted = true;
                if r.listed == 0 {
                    r.listed = now();
                }
                r.tries = 0;
                r.status.clear();
            } else if msg.starts_with("board: ")
                && !msg.contains("429")
                && !msg.contains("try again")
                // The board's server is restarting or unreachable behind its proxy.
                && !matches!(
                    msg.trim_start_matches("board: "),
                    "500" | "502" | "503" | "504"
                )
            {
                // The board's own answer. A few tries (a block may have just arrived),
                // then the offer is given up and its GNR freed.
                r.tries += 1;
                r.status = format!("Not published yet: {}", msg.trim_start_matches("board: "));
                if r.tries >= 8 {
                    m.reserve = Some(r);
                    give_up(
                        e.wallet,
                        m,
                        format!(
                            "The board did not accept this offer: {}. Its GNR is free again.",
                            msg.trim_start_matches("board: ")
                        ),
                    );
                    return;
                }
            } else if r.cm.is_none() && !msg.contains("sync") {
                // The wallet cannot gather the GNR after all.
                m.reserve = Some(r);
                give_up(e.wallet, m, format!("Not published: {msg}."));
                return;
            } else {
                r.status = format!("Not published yet: {msg}");
            }
        }
    }
    m.reserve = Some(r);
}

/// Ends a buy offer with a deposit: from now on what is at its address is sent back.
fn close_fund(m: &mut MyOffer, cancel: bool, why: &str) {
    if cancel {
        m.cancelled = true;
    }
    if let Some(f) = m.fund.as_mut()
        && f.closed.is_none()
        && !f.handed
    {
        f.closed = Some(now());
        f.last_check = 0;
        f.status = why.to_string();
    }
}

/// One look at the deposit address of a buy offer that is not listed yet. `Ok(true)`:
/// the bitcoin is there and confirmed, and the board has the offer.
fn fund_step(
    e: &mut Engine<'_, Multi>,
    base: &str,
    offer: &mut Offer,
    f: &mut Fund,
    auto: Option<&AutoCfg>,
) -> Result<bool> {
    let coins = e.deposit_coins(f.d)?;
    f.seen = coins.iter().map(|c| c.value.to_sat()).sum();
    f.confirmed = coins
        .iter()
        .filter(|c| c.height.is_some())
        .map(|c| c.value.to_sat())
        .sum();
    // A payment a little short of what was asked (a sender that took its fee out of it)
    // still does: half of what is set aside for the fee must be there.
    let enough = offer.sats + f.need.saturating_sub(offer.sats) / 2;
    if f.confirmed < enough {
        f.status = if f.seen >= enough {
            "Your bitcoin arrived. The offer is published by itself after its first confirmation (about ten minutes).".into()
        } else if f.seen > 0 {
            format!(
                "{} BTC arrived; send {} BTC more to {}.",
                btc_text(f.seen),
                btc_text(f.need - f.seen),
                f.address
            )
        } else {
            format!(
                "Waiting for your bitcoin: send {} BTC to {}. The offer is published by itself once it has one confirmation.",
                btc_text(f.need),
                f.address
            )
        };
        return Ok(false);
    }
    f.status = "Your bitcoin is confirmed. Publishing the offer.".into();
    if !offer.pow_ok_at(now()) {
        offer.seal_pow();
    }
    let mut body = serde_json::to_value(&*offer)?;
    let key = deposit::key(e.wallet.swap_seed(), f.d);
    body["funding"] = serde_json::to_value(board::sign_funding(offer, &key))?;
    if let Some(a) = auto_json(auto) {
        body["auto"] = a;
    }
    let r = post(base, "/v1/swap/offers", &body)?;
    f.marked = r["deposited"].as_bool().unwrap_or(false);
    Ok(true)
}

/// Carries a buy offer with a deposit one step further towards the board: waits for the
/// bitcoin, then posts the offer with the proof of its deposit address.
fn advance_buy(e: &mut Engine<'_, Multi>, base: &str, m: &mut MyOffer) {
    let Some(mut f) = m.fund.clone() else {
        return;
    };
    if f.posted || f.closed.is_some() || f.handed || m.cancelled || m.taken {
        return;
    }
    let t = now();
    if m.offer.expires <= t + 60 {
        close_fund(
            m,
            true,
            "The offer ran out before it could be published. What is at its address is sent back to you.",
        );
        return;
    }
    // Often at first, then once a minute while nothing has arrived.
    let every = if f.seen == 0 && f.since + 600 < t {
        60
    } else {
        20
    };
    if f.last_check + every > t {
        return;
    }
    f.last_check = t;
    match fund_step(e, base, &mut m.offer, &mut f, m.auto.as_ref()) {
        Ok(true) => {
            f.posted = true;
            f.tries = 0;
            f.status.clear();
        }
        Ok(false) => {}
        Err(x) => {
            let msg = x.to_string();
            if msg.contains("an offer with this id exists") {
                f.posted = true;
                f.tries = 0;
                f.status.clear();
            } else if msg.starts_with("board: ")
                && !msg.contains("429")
                && !msg.contains("try again")
                && !matches!(
                    msg.trim_start_matches("board: "),
                    "500" | "502" | "503" | "504"
                )
            {
                // The board's own answer. Its Bitcoin server may be a block behind this
                // wallet's: try for a while before giving up.
                f.tries += 1;
                f.status = format!("Not published yet: {}", msg.trim_start_matches("board: "));
                if f.tries >= 30 {
                    m.fund = Some(f);
                    close_fund(
                        m,
                        true,
                        &format!(
                            "The board did not accept this offer: {}. Your bitcoin is sent back to you.",
                            msg.trim_start_matches("board: ")
                        ),
                    );
                    return;
                }
            } else {
                f.status = format!("Not published yet: {msg}");
            }
        }
    }
    m.fund = Some(f);
}

/// Sends the bitcoin of buy offers that are over back to their owners' addresses, and
/// keeps looking for a few days in case a payment arrives late.
fn settle_funds(e: &mut Engine<'_, Multi>, st: &mut State) -> bool {
    let t = now();
    let mut changed = false;
    let offers = st.offers.iter_mut().filter_map(|m| {
        let MyOffer {
            fund, btc_address, ..
        } = m;
        fund.as_mut().map(|f| (f, btc_address.as_str()))
    });
    let takes = st.takes.iter_mut().filter_map(|x| {
        let MyTake {
            fund, btc_address, ..
        } = x;
        fund.as_mut().map(|f| (f, btc_address.as_str()))
    });
    for (f, back) in offers.chain(takes) {
        let Some(at) = f.closed else { continue };
        // The Bitcoin balance stays where it is.
        if f.d == deposit::BALANCE {
            continue;
        }
        if f.handed || t > at + FUND_WATCH_SECS {
            continue;
        }
        let every = if t < at + 3600 { 30 } else { 600 };
        if f.last_check + every > t {
            continue;
        }
        f.last_check = t;
        changed = true;
        match e.sweep_deposit(f.d, back) {
            Ok(Some(id)) => {
                f.returned.push(id.to_string());
                f.seen = 0;
                f.confirmed = 0;
                f.status = format!("Your bitcoin was sent back to {back} ({id}).");
            }
            Ok(None) => {
                if f.returned.is_empty() {
                    f.seen = 0;
                    f.confirmed = 0;
                    f.status = "No bitcoin is at the deposit address. If a payment still arrives there in the next three days, it is sent back to you.".into();
                }
            }
            Err(x) => {
                f.status =
                    format!("Sending your bitcoin back did not work yet ({x}); trying again.")
            }
        }
    }
    changed
}

/// GNR held back for this wallet's open sell offers (amount + fee each), so a buyer who
/// takes one always finds the coins there: sends and other offers cannot use them.
pub fn reserved(w: &Wallet) -> u64 {
    let t = now();
    load(w)
        .offers
        .iter()
        // Offers with a note of their own hold it themselves (`Wallet::holds`): their
        // GNR is not part of the spendable balance at all.
        .filter(|m| m.reserve.is_none())
        .filter(|m| {
            m.offer.side == Side::SellGnr && !m.taken && !m.cancelled && m.offer.expires > t
        })
        .map(|m| m.offer.gnr.saturating_add(krypton_wallet_core::DEFAULT_FEE))
        .fold(0u64, u64::saturating_add)
}

/// GNR set aside in notes of their own: for open sell offers (`open`), and what is
/// being released after cancelled ones, with when (`cooling`: value, unix seconds).
pub fn held(w: &Wallet) -> (u64, Vec<(u64, u64)>) {
    let (mut open, mut cooling) = (0u64, Vec::new());
    for h in w.holds() {
        let Some(n) = w.note(&h.cm) else { continue };
        match h.until {
            None => open = open.saturating_add(n.value),
            Some(t) => cooling.push((n.value, t)),
        }
    }
    cooling.sort_by_key(|c| c.1);
    (open, cooling)
}

/// Spendable GNR not reserved for open sell offers.
pub fn available(w: &Wallet) -> u64 {
    w.balance().spendable.saturating_sub(reserved(w))
}

/// Tells the board this wallet is online for its open offers (at most every 45 s).
/// Returns the ones the board no longer lists (it removed them: their maker was offline
/// too long, or the price is too far from the market), so their GNR can be freed.
fn ping(base: &str, st: &State) -> Vec<String> {
    static LAST: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    let t = now();
    let open: Vec<&MyOffer> = st
        .offers
        .iter()
        .filter(|m| !m.taken && !m.cancelled && m.offer.expires > t)
        // An offer still on its way to the board is not there to be missed.
        .filter(|m| m.reserve.as_ref().is_none_or(|r| r.posted))
        .filter(|m| m.fund.as_ref().is_none_or(|f| f.posted))
        .collect();
    let Some(first) = open.first() else {
        return vec![];
    };
    let key = format!("{base}|{}", first.offer.id);
    let mut last = LAST.get_or_init(Default::default).lock();
    if last
        .get(&key)
        .is_some_and(|i| i.elapsed() < Duration::from_secs(45))
    {
        return vec![];
    }
    let items: Vec<Value> = open
        .iter()
        .take(500)
        .map(|m| json!({"id": m.offer.id, "secret": m.cancel_secret}))
        .collect();
    match post(base, "/v1/swap/offers/ping", &json!({ "items": items })) {
        Ok(v) => {
            last.insert(key, Instant::now());
            v["missing"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        }
        Err(_) => vec![],
    }
}

fn key_of(w: &Wallet, n: u32) -> krypton_crypto::kem::SecretKey {
    offer_key(w.swap_seed(), 1_000_000u32.wrapping_add(n))
}

/// How a purchase pays: by hand once the seller's GNR is locked, from a deposit address
/// made for it (paid by the buyer first), or from the wallet's Bitcoin balance at once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TakePay {
    Later,
    Deposit,
    Balance,
}

fn take_offer(
    e: &mut Engine<'_, Multi>,
    base: &str,
    id: &str,
    btc_address: &str,
    deposit: bool,
) -> Result<(String, Option<Fund>)> {
    let raw = get(base, "/v1/swap/offers")?["offers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|o| o["id"].as_str() == Some(id))
        .context("this offer is no longer on the board")?;
    let pay = if deposit {
        TakePay::Deposit
    } else {
        TakePay::Later
    };
    take_listed(e, base, &raw, btc_address, pay, None, None)
}

/// Takes offer `id` with the wallet's Bitcoin balance, as a Quick Swap part does: buying
/// GNR pays the contract from the balance once the seller's GNR is locked (what running
/// purchases will still pay is kept aside), selling GNR is paid into the balance.
fn take_with_balance(
    e: &mut Engine<'_, Multi>,
    base: &str,
    id: &str,
) -> Result<(String, Option<Fund>)> {
    let raw = get(base, "/v1/swap/offers")?["offers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|o| o["id"].as_str() == Some(id))
        .context("this offer is no longer on the board")?;
    let offer: Offer = serde_json::from_value(raw.clone())?;
    let to = e.deposit_address(deposit::BALANCE).to_string();
    if offer.side == Side::SellGnr {
        let st = load(e.wallet);
        let b = quick::balance(e, &st)?;
        let free = b.confirmed.saturating_sub(b.committed);
        ensure!(
            free >= offer.sats.saturating_add(quick::btc_fee(e)),
            "your Bitcoin balance has {} BTC free and confirmed; this offer needs {} BTC and the network fee. Add bitcoin to your balance first (it needs one confirmation)",
            btc_text(free),
            btc_text(offer.sats)
        );
        take_listed(e, base, &raw, &to, TakePay::Balance, None, None)
    } else {
        take_listed(e, base, &raw, &to, TakePay::Later, None, None)
    }
}

/// Takes the offer the board listed as `raw`, or the part `part` (GNR, sat) of it.
fn take_listed(
    e: &mut Engine<'_, Multi>,
    base: &str,
    raw: &Value,
    btc_address: &str,
    pay: TakePay,
    part: Option<(u64, u64)>,
    quick: Option<String>,
) -> Result<(String, Option<Fund>)> {
    // Boards before 1.0.20 do not report it: treat as online.
    ensure!(
        raw["online"].as_bool().unwrap_or(true),
        "the maker of this offer is offline right now, so the swap could not start at once; pick an online offer"
    );
    let proven = raw["backed"].as_bool().unwrap_or(false);
    let prepaid_only = raw["prepaid"].as_bool().unwrap_or(false);
    let auto: Option<board::Auto> = serde_json::from_value(raw["auto"].clone()).ok();
    let offer: Offer = serde_json::from_value(raw.clone())?;
    let id = offer.id.clone();
    offer.validate(e.wallet.network().as_str(), now())?;
    ensure!(
        offer.side != Side::SellGnr || proven,
        "this offer does not prove that its GNR is there (the board it is on runs older software), so it cannot be taken"
    );
    ensure!(
        offer.btc_network == e.btc_network.to_core_arg(),
        "the offer is for Bitcoin {}",
        offer.btc_network
    );
    if let Some((g, s)) = part {
        board::part_ok(&offer, auto.as_ref(), g, s)?;
    }
    let (gnr, sats) = part.unwrap_or((offer.gnr, offer.sats));
    let part = part.filter(|p| *p != (offer.gnr, offer.sats));
    let mut st = load(e.wallet);
    ensure!(
        offer.side != Side::SellGnr || !prepaid_only || pay != TakePay::Later,
        "this seller sells only to buyers whose bitcoin is deposited first: choose \"Pay the bitcoin now\""
    );
    ensure!(
        !st.offers.iter().any(|m| m.offer.id == id),
        "this is your own offer"
    );
    ensure!(
        !st.takes.iter().any(|t| t.offer.id == id && !t.gone),
        "you already took this offer"
    );
    engine::btc_address(btc_address, e.btc_network)?;
    let n = fresh_n(&st);
    let key = key_of(e.wallet, n);
    let mut take = MyTake {
        n,
        offer: offer.clone(),
        btc_address: btc_address.trim().into(),
        swap_id: None,
        gone: false,
        fund: None,
        at: now(),
        ended: 0,
        why: String::new(),
        part,
        quick,
        promise: None,
    };
    match offer.side {
        // The maker sells GNR and we pay in advance: the bitcoin goes to an address of
        // this wallet first. The request is sent once it is there (`advance_takes`), so
        // the seller's GNR is locked only for a buyer whose bitcoin is ready.
        Side::SellGnr if pay == TakePay::Deposit => {
            let rate = e
                .btc
                .fee_rate()
                .context("the Bitcoin server did not answer; try again in a moment")?;
            let need = sats
                .checked_add(deposit::fee_reserve(rate))
                .context("amount overflow")?;
            let d = next_deposit(e, &mut st);
            let address = e.deposit_address(d).to_string();
            take.fund = Some(Fund {
                d,
                status: format!(
                    "Waiting for your bitcoin: send {} BTC to {address}. The swap starts by itself once it has one confirmation.",
                    btc_text(need)
                ),
                address,
                need,
                since: now(),
                ..Default::default()
            });
        }
        // The bitcoin is in this wallet's balance already: the request goes out at once
        // and says so, and the wallet pays the contract from the balance by itself.
        // Change and any refund come back to the balance.
        Side::SellGnr if pay == TakePay::Balance => {
            let s = e.request_from(gnr, sats, btc_address, Some(deposit::BALANCE))?;
            let code = s.code.clone().context("request code")?;
            send_note(
                base,
                &offer.key,
                &key,
                &Note::Code {
                    offer: offer.id.clone(),
                    code: code.clone(),
                },
            )?;
            take.fund = Some(Fund {
                d: deposit::BALANCE,
                address: e.deposit_address(deposit::BALANCE).to_string(),
                need: sats,
                since: now(),
                posted: true,
                ..Default::default()
            });
            take.swap_id = Some(s.id.clone());
            st.links.push(Link {
                swap_id: s.id,
                offer: offer.id.clone(),
                n,
                peer: offer.key.clone(),
                sent: vec![hash(&code)],
                gone_sent: false,
            });
        }
        // The maker sells GNR: we buy, so we start the swap with a request.
        Side::SellGnr => {
            let s = e.request(gnr, sats, btc_address)?;
            let code = s.code.clone().context("request code")?;
            send_note(
                base,
                &offer.key,
                &key,
                &Note::Code {
                    offer: offer.id.clone(),
                    code: code.clone(),
                },
            )?;
            take.swap_id = Some(s.id.clone());
            st.links.push(Link {
                swap_id: s.id,
                offer: offer.id.clone(),
                n,
                peer: offer.key.clone(),
                sent: vec![hash(&code)],
                gone_sent: false,
            });
        }
        // The maker buys GNR: we sell, and the maker answers with its request.
        Side::BuyGnr => {
            let have = available(e.wallet).saturating_sub(selling_now(&st));
            ensure!(
                have >= gnr + krypton_wallet_core::DEFAULT_FEE,
                "this wallet has {} GNR free (the rest is spoken for by your open sell offers and swaps)",
                format_amount(have)
            );
            send_note(
                base,
                &offer.key,
                &key,
                &Note::Take {
                    offer: offer.id.clone(),
                    gnr: part.map(|p| p.0),
                    sats: part.map(|p| p.1),
                },
            )?;
        }
    }
    let fund = take.fund.clone();
    st.takes.push(take);
    save(e.wallet, &st)?;
    Ok((offer.id, fund))
}

/// GNR this wallet has promised to buy offers it took and whose makers have not asked
/// for it yet: it is not free for another sale.
fn selling_now(st: &State) -> u64 {
    st.takes
        .iter()
        .filter(|x| x.offer.side == Side::BuyGnr && !x.gone && x.swap_id.is_none())
        .map(|x| x.amounts().0 + krypton_wallet_core::DEFAULT_FEE)
        .fold(0, u64::saturating_add)
}

/// Carries purchases whose bitcoin is paid in advance one step further: waits for the
/// bitcoin at the deposit address, then sends the request to the seller. If the offer is
/// gone by then, or no bitcoin arrives, the purchase is called off and what was sent goes
/// back.
fn advance_takes(e: &mut Engine<'_, Multi>, base: &str, st: &mut State) {
    let t = now();
    let mut links = Vec::new();
    let mut listing: Option<Option<Value>> = None;
    for tk in st.takes.iter_mut() {
        let Some(mut f) = tk.fund.clone() else {
            continue;
        };
        if tk.gone || tk.swap_id.is_some() || f.closed.is_some() {
            continue;
        }
        let every = if f.seen == 0 && f.since + 600 < t {
            60
        } else {
            20
        };
        if f.last_check + every > t {
            continue;
        }
        f.last_check = t;
        let end = |tk: &mut MyTake, f: &mut Fund, why: &str| {
            tk.gone = true;
            f.closed = Some(t);
            f.last_check = 0;
            f.status = why.to_string();
        };
        // The offer must still be there to be bought (one look at the board per round).
        let board = listing
            .get_or_insert_with(|| get(base, "/v1/swap/offers").ok())
            .clone();
        let seller = board.as_ref().map(|v| {
            v["offers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|o| o["id"].as_str() == Some(tk.offer.id.as_str()))
                .map(|o| {
                    o["backed"].as_bool().unwrap_or(false) && o["online"].as_bool().unwrap_or(true)
                })
        });
        if seller == Some(None) {
            end(
                tk,
                &mut f,
                "The offer is no longer on the board: someone else took it, or its seller withdrew it. What you sent goes back to you.",
            );
            tk.fund = Some(f);
            continue;
        }
        match e.deposit_coins(f.d) {
            Ok(coins) => {
                f.seen = coins.iter().map(|c| c.value.to_sat()).sum();
                f.confirmed = coins
                    .iter()
                    .filter(|c| c.height.is_some())
                    .map(|c| c.value.to_sat())
                    .sum();
            }
            Err(x) => {
                f.status = format!("The Bitcoin server did not answer ({x}); trying again.");
                tk.fund = Some(f);
                continue;
            }
        }
        let (tk_gnr, tk_sats) = tk.amounts();
        let enough = tk_sats + f.need.saturating_sub(tk_sats) / 2;
        if f.confirmed < enough {
            if f.seen == 0 && f.since + TAKE_FUND_SECS < t {
                end(
                    tk,
                    &mut f,
                    "No bitcoin arrived within two hours, so the purchase was called off. If a payment still arrives at the address in the next three days, it is sent back to you.",
                );
            } else {
                f.status = if f.seen >= enough {
                    "Your bitcoin arrived. The swap starts by itself after its first confirmation (about ten minutes).".into()
                } else if f.seen > 0 {
                    format!(
                        "{} BTC arrived; send {} BTC more to {}.",
                        btc_text(f.seen),
                        btc_text(f.need - f.seen),
                        f.address
                    )
                } else {
                    format!(
                        "Waiting for your bitcoin: send {} BTC to {}. The swap starts by itself once it has one confirmation.",
                        btc_text(f.need),
                        f.address
                    )
                };
            }
            tk.fund = Some(f);
            continue;
        }
        if seller != Some(Some(true)) {
            if f.since + TAKE_START_SECS < t {
                end(
                    tk,
                    &mut f,
                    "The swap could not start for six hours (the seller's wallet or the board could not be reached), so the purchase was called off. Your bitcoin goes back to you.",
                );
            } else {
                f.status = "Your bitcoin is confirmed. The seller's wallet (or the board) cannot be reached right now; the swap starts by itself when it is back.".into();
            }
            tk.fund = Some(f);
            continue;
        }
        match e.request_from(tk_gnr, tk_sats, &tk.btc_address, Some(f.d)) {
            Ok(s) => {
                let code = s.code.clone().unwrap_or_default();
                let key = key_of(e.wallet, tk.n);
                let note = Note::Code {
                    offer: tk.offer.id.clone(),
                    code: code.clone(),
                };
                // If it cannot be sent now, the round that forwards codes sends it.
                let sent = send_note(base, &tk.offer.key, &key, &note).is_ok();
                tk.swap_id = Some(s.id.clone());
                f.posted = true;
                f.status.clear();
                links.push(Link {
                    swap_id: s.id,
                    offer: tk.offer.id.clone(),
                    n: tk.n,
                    peer: tk.offer.key.clone(),
                    sent: if sent { vec![hash(&code)] } else { vec![] },
                    gone_sent: false,
                });
            }
            Err(x) => f.status = format!("The swap could not start yet: {x}"),
        }
        tk.fund = Some(f);
    }
    st.links.extend(links);
}

/// A report the board did not take is sent again this often, this many times (a day).
const REPORT_RETRY_SECS: u64 = 5 * 60;
const REPORT_TRIES: u32 = 288;

/// The proof that goes with the report `tr` of the finished swap `s`: its bitcoin
/// contract and the payment into it, signed with this wallet's key of the contract.
/// `None` when the swap's record does not have them (a swap from before 1.0.23).
fn swap_proof(e: &Engine<'_, Multi>, s: &engine::Swap, tr: &Trade) -> Option<board::Paid> {
    use krypton_swap::bitcoin::{OutPoint, Txid};
    use std::str::FromStr;
    let htlc = s.htlc().ok()?;
    let d = s.deposit.as_ref()?;
    let funding = OutPoint::new(Txid::from_str(&d.txid).ok()?, d.vout);
    let key = krypton_swap::derive_key(e.wallet.swap_seed(), s.index);
    let ours = krypton_swap::public_key(&key);
    if ours != htlc.claim && ours != htlc.refund {
        return None;
    }
    Some(board::sign_paid(
        tr,
        &htlc,
        e.btc_network,
        &funding,
        d.height?,
        &key,
    ))
}

fn hash(code: &str) -> String {
    hex::encode(krypton_crypto::hash::tagged(
        "Krypton/board/sent",
        code.as_bytes(),
    ))
}

/// One round: read every inbox we own, answer, forward new codes, report finished swaps.
pub fn poll(a: &App, e: &mut Engine<'_, Multi>) -> Result<()> {
    let base = board_url(a);
    let mut st = load(e.wallet);
    let t = now();
    // Offers on their way to the board. Sell offers: gather, prove, post. Buy offers
    // with a deposit: wait for the bitcoin, post.
    for m in st.offers.iter_mut() {
        advance(e, &base, m);
        advance_buy(e, &base, m);
    }
    advance_takes(e, &base, &mut st);
    // Offers the board no longer lists.
    let gone = ping(&base, &st);
    for m in st
        .offers
        .iter_mut()
        .filter(|m| !m.taken && !m.cancelled && gone.contains(&m.offer.id))
    {
        let again = m
            .reserve
            .as_ref()
            .is_some_and(|r| r.posted && r.reposts < 20 && reserve_ready(e.wallet, r));
        if again {
            // Its GNR is still there, untouched: the board lost the offer (it restarted,
            // the maker was away for long, or the chain reorganised under the proof).
            // Prove it again and post it again.
            if let Some(r) = m.reserve.as_mut() {
                r.posted = false;
                r.reposts += 1;
                r.last_try = 0;
                r.status = "The board no longer lists this offer; publishing it again.".into();
            }
        } else if m.reserve.is_some() {
            // Over: free its GNR.
            give_up(
                e.wallet,
                m,
                "The board removed this offer. Its GNR is free again.".into(),
            );
        } else if let Some(f) = m
            .fund
            .as_mut()
            .filter(|f| f.posted && f.closed.is_none() && !f.handed && f.reposts < 20)
        {
            // Its bitcoin is still at its address: post it again.
            f.posted = false;
            f.reposts += 1;
            f.last_check = 0;
            f.status = "The board no longer lists this offer; publishing it again.".into();
        } else {
            close_fund(
                m,
                true,
                "The board removed this offer. Your bitcoin is sent back to you.",
            );
            m.cancelled = true;
        }
    }
    // Sell offers that ran their time: what was listed stays committed for its week.
    for m in st.offers.iter_mut().filter(|m| {
        !m.taken
            && !m.cancelled
            && m.offer.expires <= t
            && m.reserve.as_ref().is_some_and(|r| !r.released)
    }) {
        let expires = m.offer.expires;
        if let Some(r) = m.reserve.as_mut() {
            let until = commit_until(r, expires, t);
            if let Some(cm) = r.note() {
                let _ = e.wallet.release(&cm, until);
            }
            r.released = true;
            r.free_at = until;
            r.status = format!("The offer ran its time. {}", commit_text(until));
        }
    }
    // A buy offer we took whose maker never answered: let go of it.
    for tk in st
        .takes
        .iter_mut()
        .filter(|x| x.fund.is_none() && x.swap_id.is_none() && !x.gone)
    {
        if tk.at + TAKE_ANSWER_SECS < t {
            tk.gone = true;
            tk.ended = t;
            tk.why = "The other wallet did not answer within ten minutes, so nothing was started. Nothing was locked or paid.".into();
            if tk.at > 0 {
                let key = key_of(e.wallet, tk.n);
                let note = Note::Gone {
                    offer: tk.offer.id.clone(),
                };
                let _ = send_note(&base, &tk.offer.key, &key, &note);
            }
        }
    }
    // Buy offers that ran their time: their bitcoin goes back.
    for m in st.offers.iter_mut().filter(|m| {
        !m.taken
            && !m.cancelled
            && m.offer.expires <= t
            && m.fund
                .as_ref()
                .is_some_and(|f| f.closed.is_none() && !f.handed)
    }) {
        close_fund(
            m,
            false,
            "The offer ran its time without a seller. Your bitcoin is sent back to you.",
        );
    }
    let mut keys: Vec<u32> = st
        .offers
        .iter()
        .filter(|o| !o.cancelled && (o.offer.expires + 3600 > t || o.taken))
        .map(|o| o.n)
        .collect();
    keys.extend(st.takes.iter().filter(|x| !x.gone).map(|x| x.n));
    keys.extend(st.links.iter().map(|l| l.n));
    keys.sort_unstable();
    keys.dedup();
    let swaps = engine::load(e.wallet);
    let active: std::collections::HashSet<String> = swaps
        .iter()
        .filter(|s| !s.stage.finished())
        .map(|s| s.id.clone())
        .collect();
    // Keys whose swaps are all finished need no more polling.
    keys.retain(|n| {
        let links: Vec<&Link> = st.links.iter().filter(|l| l.n == *n).collect();
        links.is_empty() || links.iter().any(|l| active.contains(&l.swap_id))
    });
    for n in keys {
        let key = key_of(e.wallet, n);
        let inbox = board::inbox_of(key.public());
        let Ok(v) = get(&base, &format!("/v1/swap/inbox/{inbox}")) else {
            continue;
        };
        for m in v["messages"].as_array().cloned().unwrap_or_default() {
            let Some(data) = m["data"].as_str() else {
                continue;
            };
            let id = hash(data);
            if st.seen.contains(&id) {
                continue;
            }
            st.seen.push(id);
            let Ok((from, body)) = board::open(&key, data) else {
                continue;
            };
            let Ok(note) = serde_json::from_slice::<Note>(&body) else {
                continue;
            };
            let from_hex = hex::encode(from.to_bytes());
            if let Err(x) = handle(e, &base, &mut st, n, &key, &from_hex, note) {
                *a.last_error.lock() = format!("swap board: {x}");
            }
        }
    }
    // Forward codes the engine produced since the last round.
    let swaps = engine::load(e.wallet);
    for l in st.links.iter_mut() {
        let Some(s) = swaps.iter().find(|s| s.id == l.swap_id) else {
            continue;
        };
        let Some(code) = &s.code else { continue };
        // GNR first, the seller's accept code stays current while its lock confirms.
        let fresh = matches!(s.stage, Stage::Requested | Stage::Accepted)
            || (s.gnr_first() && s.role == Role::GnrSeller && s.stage == Stage::Locked)
            || (s.role == Role::BtcSeller && s.stage == Stage::Deposited);
        if fresh && !l.sent.contains(&hash(code)) {
            let key = key_of(e.wallet, l.n);
            if send_note(
                &base,
                &l.peer,
                &key,
                &Note::Code {
                    offer: l.offer.clone(),
                    code: code.clone(),
                },
            )
            .is_ok()
            {
                l.sent.push(hash(code));
            }
        }
    }
    // Completed swaps count for the market price: amounts and hash lock, and from 1.0.44
    // the swap's bitcoin contract, signed with this wallet's key of it, so the board can
    // find the payment on the Bitcoin chain and weigh the swap by it.
    let share = a.settings.lock().share_prices.unwrap_or(true);
    for s in swaps.iter().filter(|s| s.stage == Stage::Done) {
        let h = hex::encode(s.hash);
        if !share || st.reported.contains(&h) {
            continue;
        }
        let tried = st.report_tries.iter().position(|(x, _, _)| *x == h);
        // (A private test chain has its blocks seconds apart.)
        let wait = if e.wallet.network() == Network::Devnet {
            10
        } else {
            REPORT_RETRY_SECS
        };
        if tried.is_some_and(|k| st.report_tries[k].1 + wait > t) {
            continue;
        }
        let mut tr = Trade {
            network: e.wallet.network().as_str().into(),
            hash: h.clone(),
            gnr: s.gnr,
            sats: s.sats,
            nonce: 0,
        };
        tr.seal_pow();
        let proof = swap_proof(e, s, &tr);
        let mut body = serde_json::to_value(&tr)?;
        if let Some(p) = &proof {
            body["paid"] = serde_json::to_value(p)?;
        }
        match post(&base, "/v1/swap/trade", &body) {
            Ok(_) => {
                st.reported.push(h.clone());
                st.report_tries.retain(|(x, _, _)| *x != h);
            }
            Err(x) => {
                // The board has not found the claim or the bitcoin yet: again later, and
                // after a day the swap is let be.
                let n = match tried {
                    Some(k) => {
                        st.report_tries[k].1 = t;
                        st.report_tries[k].2 += 1;
                        st.report_tries[k].2
                    }
                    None => {
                        st.report_tries.push((h.clone(), t, 1));
                        1
                    }
                };
                if n >= REPORT_TRIES || x.to_string().contains("bad swap proof") {
                    st.reported.push(h.clone());
                    st.report_tries.retain(|(x, _, _)| *x != h);
                }
            }
        }
    }
    // Our taken offers: shown as "in progress" while their swap runs, removed when it ends.
    for m in st
        .offers
        .iter_mut()
        .filter(|m| m.taken && !m.pending_cleared)
    {
        let Some(l) = st.links.iter().find(|l| l.offer == m.offer.id) else {
            continue;
        };
        // Over for the board once it can no longer complete: finished, or the other side
        // gave up (or never paid) before any bitcoin arrived. Locked GNR still waits for
        // its refund, but nobody should see the swap as running.
        let swap = swaps.iter().find(|s| s.id == l.swap_id);
        let over = swap.is_none_or(|s| {
            s.stage.finished()
                || (s.deposit.is_none()
                    && (l.gone_sent || s.created + engine::DEPOSIT_TIMEOUT_SECS + 3600 < t))
        });
        // The bitcoin is in its contract: only then does the swap count as running.
        let funded = swap.is_some_and(|s| s.deposit.is_some());
        if over {
            let gone = post(
                &base,
                "/v1/swap/offers/cancel",
                &json!({"id": m.offer.id, "secret": m.cancel_secret}),
            );
            m.pending_cleared = gone.is_ok();
        } else if !m.pending_sent
            || m.pending_at + PENDING_REFRESH_SECS <= t
            || (funded && !m.funded_sent)
        {
            match announce_pending(&base, m, funded) {
                Ok(()) => {
                    m.pending_sent = true;
                    m.pending_at = t;
                    m.funded_sent = funded;
                }
                Err(x) if x.to_string().contains("no such offer") => m.pending_cleared = true,
                Err(_) => {}
            }
        }
    }
    // Swaps that ended before anything was locked (cancelled or expired): tell the other
    // side, and put our own offer back on the board while it is still current.
    let mut again: Vec<MyOffer> = Vec::new();
    for l in st.links.iter_mut() {
        let Some(s) = swaps.iter().find(|s| s.id == l.swap_id) else {
            continue;
        };
        if !s.ended_unlocked() {
            continue;
        }
        if !l.gone_sent {
            let key = key_of(e.wallet, l.n);
            if send_note(
                &base,
                &l.peer,
                &key,
                &Note::Gone {
                    offer: l.offer.clone(),
                },
            )
            .is_ok()
            {
                l.gone_sent = true;
            }
        }
        if let Some(m) = st
            .offers
            .iter_mut()
            .find(|m| m.offer.id == l.offer && m.taken && !m.republished)
        {
            m.republished = true;
            // Its GNR was never locked: the note is still there, committed for its week.
            // The new offer (below) takes exactly it.
            let expires = m.offer.expires;
            if let Some(r) = m.reserve.as_mut()
                && !r.released
            {
                let until = commit_until(r, expires, t);
                if let Some(cm) = r.note() {
                    let _ = e.wallet.release(&cm, until);
                }
                r.released = true;
                r.free_at = until;
            }
            if m.offer.expires > t + 900 {
                // The new offer takes the deposit over: this record lets go of it.
                if let Some(f) = m.fund.as_mut().filter(|f| f.closed.is_none()) {
                    f.handed = true;
                }
                again.push(m.clone());
            }
        }
    }
    // Buy offers whose swap is over: what is left at their deposit address goes back.
    for m in st.offers.iter_mut().filter(|m| m.taken) {
        if m.fund
            .as_ref()
            .is_none_or(|f| f.closed.is_some() || f.handed)
        {
            continue;
        }
        let mut links = st.links.iter().filter(|l| l.offer == m.offer.id).peekable();
        if links.peek().is_none() {
            continue;
        }
        let over = links.all(|l| {
            swaps
                .iter()
                .find(|s| s.id == l.swap_id)
                .is_none_or(|s| s.stage.finished())
        });
        if over {
            close_fund(
                m,
                false,
                "The swap of this offer is over. What is left at its deposit address is sent back to you.",
            );
        }
    }
    // The same for purchases paid in advance.
    for tk in st.takes.iter_mut() {
        let (Some(f), Some(id)) = (tk.fund.as_mut(), tk.swap_id.as_ref()) else {
            continue;
        };
        if f.closed.is_none()
            && swaps
                .iter()
                .find(|s| s.id == *id)
                .is_none_or(|s| s.stage.finished())
        {
            f.closed = Some(t);
            f.last_check = 0;
            f.status =
                "The swap is over. What is left at its deposit address is sent back to you.".into();
        }
    }
    list_rests(e, &mut st, &swaps);
    end_promises(&base, &mut st, &swaps);
    part_answers(e, &st, &swaps, t);
    advance_quick(e, &base, &mut st);
    follow_market(e, &base, &mut st);
    settle_funds(e, &mut st);
    mirror_publish(e, &base, &mut st, &swaps);
    let n = st.seen.len();
    if n > 1000 {
        st.seen.drain(..n - 1000);
    }
    save(e.wallet, &st)?;
    for m in again {
        let pay = match &m.fund {
            Some(f) if f.handed => Pay::Reuse(Fund {
                d: f.d,
                address: f.address.clone(),
                need: f.need,
                since: now(),
                status: "Publishing the offer again with the bitcoin you deposited.".into(),
                ..Default::default()
            }),
            _ => Pay::Later,
        };
        let handed = matches!(pay, Pay::Reuse(_));
        let r = publish_offer(
            e,
            &base,
            m.offer.side,
            m.offer.gnr,
            m.offer.sats,
            &m.btc_address,
            m.offer.expires.saturating_sub(now()),
            pay,
            m.prepaid_only,
            m.follow.clone(),
            m.auto.clone(),
        );
        if r.is_err() && handed {
            // No new offer has the deposit: the old one keeps it, and it goes back.
            let mut st = load(e.wallet);
            if let Some(o) = st.offers.iter_mut().find(|o| o.offer.id == m.offer.id) {
                if let Some(f) = o.fund.as_mut() {
                    f.handed = false;
                }
                close_fund(
                    o,
                    false,
                    "The offer could not be published again. Your bitcoin is sent back to you.",
                );
            }
            save(e.wallet, &st)?;
        }
    }
    Ok(())
}

fn handle(
    e: &mut Engine<'_, Multi>,
    base: &str,
    st: &mut State,
    n: u32,
    key: &krypton_crypto::kem::SecretKey,
    from: &str,
    note: Note,
) -> Result<()> {
    match note {
        Note::Code { offer, code } => {
            // A swap already linked to this sender: its next code, for that swap only.
            if st.links.iter().any(|l| l.n == n && l.peer == from) {
                let id = Msg::decode(&code)?.id().to_string();
                ensure!(
                    st.links
                        .iter()
                        .any(|l| l.n == n && l.peer == from && l.swap_id == id),
                    "a code for another swap"
                );
                e.receive(&code)?;
                return Ok(());
            }
            let msg = Msg::decode(&code)?;
            let (rid, gnr, sats) = match msg {
                Msg::Request2 { id, gnr, sats, .. } => (id, gnr, sats),
                // A wallet before 1.0.23 (bitcoin first): it must update before trading.
                Msg::Request { .. } => {
                    let _ = send_note(base, from, key, &Note::Gone { offer });
                    bail!(engine::UPDATE_HINT)
                }
                _ => bail!("unexpected code"),
            };
            // Someone wants to buy our GNR offer.
            if let Some(mo) = st
                .offers
                .iter_mut()
                .find(|o| o.n == n && o.offer.id == offer && o.offer.side == Side::SellGnr)
            {
                let note = mo.reserve.as_ref().and_then(Reserve::note);
                // The swap of this very request may be in the wallet already: the wallet
                // was closed after it accepted the request and before this round was
                // written down. That swap carries on (its GNR may be locked by now) and
                // its codes must reach the buyer; it is not refused.
                let links = &st.links;
                let known = engine::load(e.wallet).into_iter().find(|s| {
                    s.id == rid
                        && s.role == Role::GnrSeller
                        && !s.stage.finished()
                        && (s.gnr, s.sats) == (gnr, sats)
                        && s.from_note == note.map(hex::encode)
                        && !links.iter().any(|l| l.swap_id == s.id)
                });
                let s = match known {
                    Some(s) => s,
                    None => {
                        if mo.taken || mo.cancelled {
                            return send_note(base, from, key, &Note::Gone { offer });
                        }
                        // The whole offer, or for a Quick Swap offer a part of it at its
                        // price.
                        let auto = mo.auto.as_ref().map(|a| board::Auto { min: a.min });
                        if let Err(x) = board::part_ok(&mo.offer, auto.as_ref(), gnr, sats) {
                            let _ = send_note(base, from, key, &Note::Gone { offer });
                            return Err(x.context("a request with other amounts than the offer"));
                        }
                        // An offer still on its way to the board cannot have been taken.
                        ensure!(
                            mo.reserve.as_ref().is_none_or(|r| r.posted),
                            "a request for an offer that is not published yet"
                        );
                        // A buyer whose bitcoin waits at an address of its own wallet
                        // says so; the Bitcoin chain shows whether that is true, and the
                        // board whether other sellers count on the same bitcoin.
                        let (shown, promise) = deposit_promised(
                            e,
                            base,
                            &code,
                            sats,
                            &mo.offer.id,
                            Some(mo.cancel_secret.as_str()),
                        );
                        let refuse = if mo.prepaid_only && !shown {
                            Some(
                                "a request for an offer that is only for buyers whose bitcoin is deposited first",
                            )
                        } else if !shown && unpaid_locks(e.wallet) >= engine::MAX_UNPAID_LOCKS {
                            Some(
                                "a request while GNR is already locked for buyers who have not paid",
                            )
                        } else {
                            None
                        };
                        if let Some(why) = refuse {
                            release_promise(base, promise.as_ref());
                            let _ = send_note(base, from, key, &Note::Gone { offer });
                            bail!(why);
                        }
                        // The lock spends the note set aside for this offer, nothing
                        // else (a part leaves the rest of it as change). The offer counts
                        // as taken only once the swap exists.
                        match e.accept_from(&code, &mo.btc_address, note, shown) {
                            Ok(s) => {
                                mo.promise = promise;
                                s
                            }
                            Err(x) => {
                                release_promise(base, promise.as_ref());
                                let _ = send_note(base, from, key, &Note::Gone { offer });
                                return Err(x);
                            }
                        }
                    }
                };
                mo.part = ((gnr, sats) != (mo.offer.gnr, mo.offer.sats)).then_some((gnr, sats));
                mo.taken = true;
                if announce_pending(base, mo, false).is_ok() {
                    mo.pending_sent = true;
                    mo.pending_at = now();
                }
                st.links.push(Link {
                    swap_id: s.id,
                    offer,
                    n,
                    peer: from.into(),
                    sent: vec![],
                    gone_sent: false,
                });
                return Ok(());
            }
            // The maker of a buy offer we took answers with its request.
            if let Some(tk) = st.takes.iter_mut().find(|x| {
                x.n == n
                    && x.offer.id == offer
                    && x.swap_id.is_none()
                    && !x.gone
                    && x.offer.side == Side::BuyGnr
            }) {
                ensure!(
                    from == tk.offer.key,
                    "a request from someone other than the maker"
                );
                // Exactly what was taken: the part, or the whole offer.
                if (gnr, sats) != tk.amounts() {
                    tk.gone = true;
                    tk.ended = now();
                    tk.why = "The other wallet asked for other amounts than you took, so nothing was started. Nothing was locked or paid.".into();
                    let _ = send_note(base, from, key, &Note::Gone { offer });
                    bail!("a request with other amounts than the offer");
                }
                let addr = tk.btc_address.clone();
                let (shown, promise) = deposit_promised(e, base, &code, sats, &offer, None);
                // A Quick Swap sells only to a buyer whose bitcoin is there, and not
                // promised to others already.
                if tk.quick.is_some() && !shown {
                    release_promise(base, promise.as_ref());
                    tk.gone = true;
                    tk.ended = now();
                    tk.why =
                        "The buyer's bitcoin was not where its wallet said, so nothing was locked."
                            .into();
                    let _ = send_note(base, from, key, &Note::Gone { offer });
                    bail!("a Quick Swap buyer whose bitcoin is not there");
                }
                let s = match e.accept_from(&code, &addr, None, shown) {
                    Ok(s) => s,
                    Err(x) => {
                        release_promise(base, promise.as_ref());
                        return Err(x);
                    }
                };
                tk.promise = promise;
                tk.swap_id = Some(s.id.clone());
                st.links.push(Link {
                    swap_id: s.id,
                    offer,
                    n,
                    peer: from.into(),
                    sent: vec![],
                    gone_sent: false,
                });
                return Ok(());
            }
            // A listing of an offer that has followed the market since: it is gone, and
            // the other wallet should know at once rather than wait for an answer.
            if st.offers.iter().any(|o| o.n == n && o.was.contains(&offer)) {
                return send_note(base, from, key, &Note::Gone { offer });
            }
            bail!("a code for an offer this wallet does not know")
        }
        Note::Take {
            offer,
            gnr: part_gnr,
            sats: part_sats,
        } => {
            let Some(i) = st
                .offers
                .iter()
                .position(|o| o.n == n && o.offer.id == offer && o.offer.side == Side::BuyGnr)
            else {
                if st.offers.iter().any(|o| o.n == n && o.was.contains(&offer)) {
                    return send_note(base, from, key, &Note::Gone { offer });
                }
                bail!("a take for an unknown offer");
            };
            if st.offers[i].taken || st.offers[i].cancelled {
                return send_note(base, from, key, &Note::Gone { offer });
            }
            // An offer still waiting for its bitcoin cannot have been taken.
            ensure!(
                st.offers[i].fund.as_ref().is_none_or(|f| f.posted),
                "a take for an offer that is not published yet"
            );
            // With a deposit, the swap pays the contract from it by itself.
            let fund = st.offers[i]
                .fund
                .as_ref()
                .filter(|f| f.closed.is_none() && !f.handed)
                .map(|f| f.d);
            // The whole offer, or a part of a Quick Swap offer at its price. A part's
            // payment leaves the rest of the bitcoin at a new deposit address, for the
            // rest of the offer.
            let whole = (st.offers[i].offer.gnr, st.offers[i].offer.sats);
            let (gnr, sats) = match (part_gnr, part_sats) {
                (Some(g), Some(s)) => (g, s),
                _ => whole,
            };
            let auto = st.offers[i]
                .auto
                .as_ref()
                .map(|a| board::Auto { min: a.min });
            if let Err(x) = board::part_ok(&st.offers[i].offer, auto.as_ref(), gnr, sats) {
                let _ = send_note(base, from, key, &Note::Gone { offer });
                return Err(x.context("a take for other amounts than the offer"));
            }
            let part = (gnr, sats) != whole;
            if part && fund.is_none() {
                let _ = send_note(base, from, key, &Note::Gone { offer });
                bail!("a part of an offer whose bitcoin is not deposited");
            }
            let rest = part.then(|| next_deposit(e, st));
            let mo = &mut st.offers[i];
            // The offer counts as taken only once its swap exists.
            let s = match e.request_with(gnr, sats, &mo.btc_address, fund, rest) {
                Ok(s) => s,
                Err(x) => {
                    let _ = send_note(base, from, key, &Note::Gone { offer });
                    return Err(x);
                }
            };
            mo.part = part.then_some((gnr, sats));
            mo.rest_deposit = rest;
            mo.taken = true;
            if announce_pending(base, mo, false).is_ok() {
                mo.pending_sent = true;
                mo.pending_at = now();
            }
            st.links.push(Link {
                swap_id: s.id,
                offer,
                n,
                peer: from.into(),
                sent: vec![],
                gone_sent: false,
            });
            Ok(())
        }
        Note::Gone { offer } => {
            for tk in st
                .takes
                .iter_mut()
                .filter(|x| x.n == n && x.offer.id == offer)
            {
                tk.gone = true;
            }
            // The other side of a swap in progress cancelled (or let it expire): end ours
            // too while nothing is locked. Once coins are locked, the timelocks decide.
            for l in st
                .links
                .iter_mut()
                .filter(|l| l.n == n && l.peer == from && l.offer == offer)
            {
                l.gone_sent = true;
                let _ = e.cancel(
                    &l.swap_id,
                    "The other side cancelled the swap. Nothing was locked.",
                );
            }
            Ok(())
        }
        // Coin swaps have their own keys (`crate::coins`).
        _ => Ok(()),
    }
}

pub fn tick(a: &App, e: &mut Engine<'_, Multi>) {
    if let Err(x) = poll(a, e) {
        *a.last_error.lock() = format!("swap board: {x}");
    }
}

/// The address a request says its bitcoin waits at (an address of the buyer's own
/// wallet, signed by that address's key), and what the Bitcoin chain has there,
/// confirmed and unspent.
fn deposit_seen(
    e: &Engine<'_, Multi>,
    code: &str,
) -> Option<(krypton_swap::bitcoin::Address, u64)> {
    let Ok(Some(a)) = engine::prepaid_address(code, e.btc_network) else {
        return None;
    };
    // A Bitcoin server that fails once is asked again: a refusal sends the buyer away.
    for attempt in 0..3 {
        match e.btc.unspent(&a) {
            Ok(v) => {
                let have = v
                    .iter()
                    .filter(|u| u.height.is_some())
                    .map(|u| u.value.to_sat())
                    .sum::<u64>();
                return Some((a, have));
            }
            Err(_) if attempt < 2 => std::thread::sleep(Duration::from_secs(2)),
            Err(_) => {}
        }
    }
    None
}

/// Whether a request shows its bitcoin (`deposit_seen`, at least `sats`), counting what
/// the board says the same deposit is promised to other sellers (1.0.46). A buyer with
/// bitcoin for one swap then cannot have the GNR of many sellers locked at once. Returns
/// the promise made on the board (request id, release secret), to be taken back once the
/// swap is paid or over. A board that cannot count (older, unreachable) leaves the chain's
/// word as it is.
fn deposit_promised(
    e: &Engine<'_, Multi>,
    base: &str,
    code: &str,
    sats: u64,
    offer: &str,
    cancel_secret: Option<&str>,
) -> (bool, Option<(String, String)>) {
    let Some((_, have)) = deposit_seen(e, code) else {
        return (false, None);
    };
    if have < sats {
        return (false, None);
    }
    let secret: [u8; 32] = rand::random();
    let Ok(Some(p)) = engine::promise_of(code, board::release_hash(&secret)) else {
        return (true, None);
    };
    let id = p.id.clone();
    let mut body = serde_json::to_value(&p).unwrap_or_default();
    body["offer"] = json!(offer);
    if let Some(s) = cancel_secret {
        body["secret"] = json!(s);
    }
    match post(base, "/v1/swap/promise", &body) {
        Ok(v) => {
            let Some(promised) = v["promised"].as_u64() else {
                return (false, None);
            };
            let made = Some((id, hex::encode(secret)));
            if promised > have {
                release_promise(base, made.as_ref());
                (false, None)
            } else {
                (true, made)
            }
        }
        // A board from before 1.0.46 has no promises: the chain's word stands, as it did.
        // Any other failure (the board busy, unreachable): no lock without the count.
        Err(x) if x.to_string().contains("404") || x.to_string().contains("Not Found") => {
            (true, None)
        }
        Err(_) => (false, None),
    }
}

/// Takes a promise back from the board (its swap was paid, or is over).
fn release_promise(base: &str, p: Option<&(String, String)>) {
    if let Some((id, secret)) = p {
        let _ = post(
            base,
            "/v1/swap/promise/release",
            &json!({"id": id, "secret": secret}),
        );
    }
}

/// GNR locks this wallet holds, or is about to make, for buyers whose bitcoin it has not
/// seen and who have not paid.
fn unpaid_locks(w: &Wallet) -> usize {
    engine::load(w)
        .iter()
        .filter(|s| {
            s.role == Role::GnrSeller && !s.stage.finished() && s.deposit.is_none() && !s.prepaid
        })
        .count()
}

/// Swaps whose codes travel through the board.
pub fn linked_ids(w: &Wallet) -> std::collections::HashSet<String> {
    load(w).links.into_iter().map(|l| l.swap_id).collect()
}

/// Swaps whose other side said it gave up (or was told that we did).
pub fn gone_ids(w: &Wallet) -> std::collections::HashSet<String> {
    load(w)
        .links
        .into_iter()
        .filter(|l| l.gone_sent)
        .map(|l| l.swap_id)
        .collect()
}

/// This wallet's offers that are on the board now, plus takes still waiting for an answer.
pub fn open_offers(w: &Wallet) -> usize {
    let st = load(w);
    let t = now();
    st.offers
        .iter()
        .filter(|o| !o.cancelled && !o.taken && o.offer.expires > t)
        .count()
        + st.takes
            .iter()
            .filter(|x| !x.gone && x.swap_id.is_none())
            .count()
}

/// Whether this wallet has anything on the board to watch.
pub fn has_work(w: &Wallet) -> bool {
    let st = load(w);
    let t = now();
    st.offers
        .iter()
        .any(|o| !o.cancelled && !o.taken && o.offer.expires > t)
        || st.takes.iter().any(|x| !x.gone && x.swap_id.is_none())
        || !st.links.is_empty()
        || quick::busy(&st)
        // Bitcoin of a buy offer that is over may still have to go back.
        || st
            .offers
            .iter()
            .filter_map(|o| o.fund.as_ref())
            .chain(st.takes.iter().filter_map(|x| x.fund.as_ref()))
            .any(|f| !f.handed && f.closed.is_some_and(|at| at + FUND_WATCH_SECS > t))
}

/// Whether a quick board round (every few seconds) is worth it: someone may be taking an
/// open offer or answering a take right now. Capped so a wallet with very many offers
/// stays within the board's rate limits (the regular round still serves them).
pub fn quick_work(w: &Wallet) -> bool {
    let st = load(w);
    let t = now();
    let done: std::collections::HashSet<String> = engine::load(w)
        .into_iter()
        .filter(|s| {
            s.stage.finished()
                || !matches!(
                    s.stage,
                    Stage::Requested | Stage::Accepted | Stage::AwaitDeposit
                )
        })
        .map(|s| s.id)
        .collect();
    let n = st
        .offers
        .iter()
        .filter(|o| !o.cancelled && !o.taken && o.offer.expires > t)
        .count()
        + st.takes
            .iter()
            .filter(|x| !x.gone && x.swap_id.is_none())
            .count()
        + st.links
            .iter()
            .filter(|l| !done.contains(&l.swap_id))
            .count();
    (n > 0 && n <= 20) || quick::busy(&st)
}

// HTTP API for the Swap tab.

pub async fn list(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let base = board_url(&a);
        let raw = get(&base, "/v1/swap/offers").unwrap_or_default();
        // What a board lists reaches the page: each offer is read on its own and only one
        // with an id in the form every wallet makes is kept.
        let offers: Vec<Offer> = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| serde_json::from_value::<Offer>(o.clone()).ok())
            .filter(|o| swap_id_ok(&o.id))
            .collect();
        let online: std::collections::HashMap<String, bool> = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| {
                Some((
                    o["id"].as_str()?.to_string(),
                    o["online"].as_bool().unwrap_or(true),
                ))
            })
            .collect();
        // Sell offers only for buyers whose bitcoin is deposited first (1.0.43).
        let prepaid: std::collections::HashSet<String> = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["prepaid"].as_bool().unwrap_or(false))
            .filter_map(|o| o["id"].as_str().map(String::from))
            .collect();
        // Buy offers whose bitcoin the board has found at its deposit address (1.0.41).
        let deposited: std::collections::HashSet<String> = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["deposited"].as_bool().unwrap_or(false))
            .filter_map(|o| o["id"].as_str().map(String::from))
            .collect();
        // Sell offers whose GNR the board has checked (boards from 1.0.40).
        let proven: std::collections::HashSet<String> = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["backed"].as_bool().unwrap_or(false))
            .filter_map(|o| o["id"].as_str().map(String::from))
            .collect();
        let t0 = now();
        let in_progress: Vec<Value> = raw["in_progress"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let id = p["id"].as_str().filter(|id| swap_id_ok(id))?;
                let side: Side = serde_json::from_value(p["side"].clone()).ok()?;
                let (g, s) = (
                    p["gnr"].as_u64().unwrap_or(0),
                    p["sats"].as_u64().unwrap_or(0).min(MAX_OFFER_SATS),
                );
                let since = p["since"].as_u64().unwrap_or(t0);
                Some(json!({
                    "id": id, "side": side, "gnr": g, "sats": s, "since": since,
                    "funded": p["funded"].as_bool().unwrap_or(false),
                    "part": p["part"].as_bool().unwrap_or(false),
                    "gnr_text": format_amount(g),
                    "btc_text": Amount::from_sat(s).to_string_in(Denomination::Bitcoin),
                    "price": if g > 0 { s as f64 / (g as f64 / 1e8) } else { 0.0 },
                    "age": t0.saturating_sub(since),
                }))
            })
            .collect();
        let (st, quick_list) = a
            .wallet
            .lock()
            .as_ref()
            .map(|w| {
                let st = load(w);
                let q = quick::quick_view(w, &st);
                (st, q)
            })
            .unwrap_or_default();
        let mine: Vec<&String> = st.offers.iter().map(|o| &o.offer.id).collect();
        // Quick Swap offers on the board (1.0.46): sold or bought in parts by themselves.
        let autos: std::collections::HashMap<String, u64> = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| Some((o["id"].as_str()?.to_string(), o["auto"]["min"].as_u64()?)))
            .collect();
        let t = now();
        let view = |o: &Offer| {
            json!({
                "id": o.id, "side": o.side, "gnr": o.gnr, "sats": o.sats,
                "gnr_text": format_amount(o.gnr),
                "btc_text": Amount::from_sat(o.sats).to_string_in(Denomination::Bitcoin),
                "price": o.price(), "expires_in": o.expires.saturating_sub(t),
                "mine": mine.contains(&&o.id),
                "online": mine.contains(&&o.id) || online.get(&o.id).copied().unwrap_or(false),
                "backed": proven.contains(&o.id),
                "deposited": deposited.contains(&o.id),
                "prepaid": prepaid.contains(&o.id),
                "auto": autos.get(&o.id).map(|m| json!({"min": m, "min_text": format_amount(*m)})),
            })
        };
        // Fresh from the board (an offer just published shows at once), with the cached
        // BTC price.
        let mut market = get(&base, "/v1/swap/market").unwrap_or_else(|_| cached_market());
        market["btc_usd"] = cached_market()["btc_usd"].clone();
        if market["quick"].is_object()
            && let Some(listing) = raw["offers"].as_array()
        {
            let mp = market_price(&market["market"]);
            let band = quick::guard_band(&market["market"]);
            quick::guard_summary(&mut market["quick"], listing, mp, band, &mine);
        }
        Ok(json!({
            "board": base,
            "offers": offers.iter().map(view).collect::<Vec<_>>(),
            "in_progress": in_progress,
            "my_offers": st.offers.iter().rev().take(200).map(|m| {
                let mut v = view(&m.offer);
                if m.prepaid_only {
                    v["prepaid"] = json!(true);
                }
                if let Some(f) = &m.follow {
                    v["follow"] = json!({"limit": f.limit, "moves": f.moves, "at": f.at, "margin": f.margin * 100.0, "trigger": f.trigger});
                }
                if let Some(why) = &m.stopped {
                    v["stopped"] = json!(why);
                }
                if let Some(x) = &m.auto {
                    v["auto"] = json!({"min": x.min, "min_text": format_amount(x.min), "group": x.group});
                }
                if let Some((g, s)) = m.part {
                    v["part"] = json!({"gnr_text": format_amount(g), "btc_text": btc_text(s)});
                }
                v["taken"] = json!(m.taken); v["cancelled"] = json!(m.cancelled);
                v["expired"] = json!(m.offer.expires <= t);
                if let Some(r) = &m.reserve {
                    // On its way to the board: gathering the GNR, or proving it.
                    v["preparing"] = json!(!r.posted && !m.cancelled && !m.taken && m.offer.expires > t);
                    v["status"] = json!(r.status);
                    v["free_at"] = json!(r.free_at.filter(|u| *u > t));
                }
                if let Some(f) = &m.fund {
                    // Waiting for its bitcoin, or for the board.
                    v["preparing"] = json!(!f.posted && f.closed.is_none() && !f.handed && !m.cancelled && !m.taken && m.offer.expires > t);
                    v["status"] = json!(f.status);
                    v["fund"] = json!({
                        "address": f.address, "need": f.need, "need_text": btc_text(f.need),
                        "seen": f.seen, "seen_text": btc_text(f.seen), "confirmed": f.confirmed,
                        "posted": f.posted, "marked": f.marked,
                        "closed": f.closed.is_some(), "handed": f.handed, "returned": f.returned,
                        // Ended lately: still shown, so that what became of the bitcoin is seen.
                        "recent": f.closed.is_some_and(|at| at + 24 * 3600 > t),
                        "back_to": m.btc_address,
                    });
                }
                v
            }).collect::<Vec<_>>(),
            // Purchases whose bitcoin is paid in advance: waiting for it, or ended lately.
            "my_takes": st.takes.iter().rev().filter_map(|x| {
                let Some(f) = x.fund.as_ref() else {
                    // A buy offer we took (we sell): waiting for its maker's wallet to
                    // answer, or let go of lately.
                    let waiting = !x.gone && x.swap_id.is_none();
                    let recent = x.gone && x.swap_id.is_none() && x.ended + 24 * 3600 > t;
                    return (waiting || recent).then(|| json!({
                        "id": x.offer.id, "sats": x.offer.sats,
                        "gnr_text": format_amount(x.offer.gnr),
                        "btc_text": btc_text(x.offer.sats),
                        "sell": x.offer.side == Side::BuyGnr,
                        "waiting": waiting,
                        "status": if waiting {
                            "Waiting for the other wallet to answer. It answers within seconds when it is online; if it does not within ten minutes, this ends by itself. Nothing is locked or paid yet."
                        } else {
                            x.why.as_str()
                        },
                    }));
                };
                let waiting = !x.gone && x.swap_id.is_none() && f.closed.is_none();
                let recent = f.closed.is_some_and(|at| at + 24 * 3600 > t) && x.swap_id.is_none();
                (waiting || recent).then(|| json!({
                    "id": x.offer.id, "sats": x.offer.sats,
                    "gnr_text": format_amount(x.offer.gnr),
                    "btc_text": btc_text(x.offer.sats),
                    "waiting": waiting, "status": f.status,
                    "fund": {
                        "address": f.address, "need": f.need, "need_text": btc_text(f.need),
                        "seen": f.seen, "seen_text": btc_text(f.seen), "confirmed": f.confirmed,
                        "closed": f.closed.is_some(), "returned": f.returned,
                        "back_to": x.btc_address, "take": true,
                    },
                }))
            }).take(20).collect::<Vec<_>>(),
            "quick": quick_list,
            "hold_secs": COMMIT_SECS,
            "follow": {"every_secs": FOLLOW_EVERY_SECS, "min_move": FOLLOW_MIN_MOVE, "max_step": FOLLOW_MAX_STEP, "min_swaps": FOLLOW_MIN_SWAPS},
            "market": market,
        }))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct PublishReq {
    side: Side,
    gnr: String,
    btc: String,
    address: String,
    /// How long the offer stays on the board, 1 hour to a year (24 hours if not given).
    #[serde(default)]
    hours: Option<u64>,
    /// Confirms with the wallet's password (a sell offer commits GNR).
    #[serde(default)]
    password: String,
    /// A buy offer: pay the bitcoin now to an address of this wallet, so a seller's take
    /// is paid by itself (the default). `false`: pay by hand when a seller takes it.
    #[serde(default)]
    deposit: Option<bool>,
    /// A sell offer: lock the GNR only for a buyer whose bitcoin is already deposited
    /// (the default), so that nobody can take the offer and leave the GNR locked unpaid.
    #[serde(default)]
    prepaid_only: Option<bool>,
    /// The offer follows the market price (1.0.44). A sell offer then names its GNR and
    /// a buy offer its bitcoin; the other amount comes from the market price. `limit` is
    /// the price in sat per GNR the offer never goes below (sell) or above (buy).
    #[serde(default)]
    follow: Option<bool>,
    #[serde(default)]
    limit: Option<f64>,
    /// A following offer (1.0.48): percent above (or, negative, below) the market price,
    /// and the market price (sat per GNR) at which it is taken off the board by itself.
    #[serde(default)]
    margin: Option<f64>,
    #[serde(default)]
    trigger: Option<f64>,
    /// Quick Swap (1.0.46): the offer is swapped in parts by itself, with anyone; a buy
    /// offer is paid from the wallet's Bitcoin balance. `min`: the smallest part (GNR).
    #[serde(default)]
    auto: Option<bool>,
    #[serde(default)]
    min: Option<String>,
    /// The wallet's Bitcoin balance (1.0.52): a sell offer is paid into it, a buy offer is
    /// paid from it at once, and what comes back from a buy offer returns to it. `address`
    /// is not used then.
    #[serde(default)]
    balance: bool,
}

/// Pays the deposit of this wallet's new buy offer `id` from the Bitcoin balance, as the lots
/// of a Quick Swap buy offer are paid. The offer's record is there before the payment, so the
/// bitcoin is never at a deposit nothing watches; a payment that fails leaves no offer.
fn fund_from_balance(e: &mut Engine<'_, Multi>, id: &str) -> Result<()> {
    let mut st = load(e.wallet);
    let i = st
        .offers
        .iter()
        .position(|m| m.offer.id == id)
        .context("offer record")?;
    let f = st.offers[i]
        .fund
        .clone()
        .context("the offer has no deposit")?;
    let b = quick::balance(e, &st)?;
    let free = b.confirmed.saturating_sub(b.committed);
    let ok = free >= f.need.saturating_add(quick::btc_fee(e));
    let to = e.deposit_address(f.d);
    let paid = ok
        .then(|| e.pay_from(deposit::BALANCE, &[(to, f.need)], deposit::BALANCE))
        .transpose();
    if !ok || paid.is_err() {
        st.offers.remove(i);
        save(e.wallet, &st)?;
        ensure!(
            ok,
            "your Bitcoin balance has {} BTC free and confirmed; this offer needs {} BTC and the network fee. Add bitcoin to your balance first (it needs one confirmation)",
            btc_text(free),
            btc_text(f.need)
        );
        paid?;
    }
    if let Some(fund) = st.offers[i].fund.as_mut() {
        fund.status = "Paid from your Bitcoin balance; the offer is published once that has one confirmation (about ten minutes).".into();
    }
    save(e.wallet, &st)
}

pub async fn publish(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<PublishReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        let btc = |s: &str| -> Result<u64> {
            Ok(Amount::from_str_in(s.trim(), Denomination::Bitcoin)
                .map_err(|e| anyhow!("invalid BTC amount: {e}"))?
                .to_sat())
        };
        let (gnr, sats, follow) = if r.follow.unwrap_or(false) {
            let limit = r
                .limit
                .filter(|l| l.is_finite() && *l > 0.0)
                .context(match r.side {
                    Side::SellGnr => "say the lowest price this offer may go to",
                    Side::BuyGnr => "say the highest price this offer may go to",
                })?;
            let base = board_url(&a);
            let market = get(&base, "/v1/swap/market")
                .ok()
                .and_then(|m| market_price(&m["market"]))
                .or_else(|| market_price(&cached_market()["market"]))
                .context("there is no market price to follow yet: no swap has completed lately. Publish at a price of your own")?;
            let margin = r.margin.unwrap_or(0.0) / 100.0;
            ensure!(
                margin.is_finite() && margin.abs() <= FOLLOW_MAX_MARGIN + 1e-9,
                "the offer may stand at most 25 % above or below the market price"
            );
            let trigger = r.trigger.filter(|t| *t > 0.0);
            if let Some(t) = trigger {
                ensure!(t.is_finite(), "invalid trigger price");
                ensure!(
                    match r.side {
                        Side::SellGnr => t < market,
                        Side::BuyGnr => t > market,
                    },
                    match r.side {
                        Side::SellGnr => "the trigger price of a sell offer must be below the market price, or the offer would come down at once",
                        Side::BuyGnr => "the trigger price of a buy offer must be above the market price, or the offer would come down at once",
                    }
                );
            }
            let f = Follow {
                limit,
                at: now(),
                moves: 0,
                margin,
                trigger,
            };
            let (gnr, sats) = match r.side {
                Side::SellGnr => (parse_amount(&r.gnr).context("invalid GNR amount")?, 0),
                Side::BuyGnr => (0, btc(&r.btc)?),
            };
            let (gnr, sats) = amounts_at(r.side, gnr, sats, follow_price(r.side, &f, market))
                .context("the amount is too small or too large for an offer at this price")?;
            (gnr, sats, Some(f))
        } else {
            (
                parse_amount(&r.gnr).context("invalid GNR amount")?,
                btc(&r.btc)?,
                None,
            )
        };
        let secs = r.hours.unwrap_or(24).clamp(1, board::MAX_OFFER_SECS / 3600) * 3600;
        if r.auto.unwrap_or(false) {
            let min = match r.min.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(m) => Some(parse_amount(m).context("invalid smallest part")?),
                None => None,
            };
            let ids = with_engine(&a, false, |e, base| {
                let to = if r.balance {
                    e.deposit_address(deposit::BALANCE).to_string()
                } else {
                    r.address.clone()
                };
                publish_lots(e, base, r.side, gnr, sats, &to, secs, follow, min)
            })?;
            return Ok(json!({"id": ids[0], "ids": ids, "lots": ids.len(), "listed": false, "fund": null, "auto": true}));
        }
        with_engine(&a, false, |e, base| {
            let to = if r.balance {
                e.deposit_address(deposit::BALANCE).to_string()
            } else {
                r.address.trim().to_string()
            };
            let o = publish_offer(
                e,
                base,
                r.side,
                gnr,
                sats,
                &to,
                secs,
                if r.side == Side::BuyGnr && (r.balance || r.deposit.unwrap_or(true)) {
                    Pay::Deposit
                } else {
                    Pay::Later
                },
                r.prepaid_only.unwrap_or(true),
                follow,
                None,
            )?;
            if r.balance && r.side == Side::BuyGnr {
                fund_from_balance(e, &o.id)?;
                return Ok(json!({"id": o.id, "listed": false, "fund": null, "from_balance": true}));
            }
            // A sell offer is listed once its GNR is set aside and proven: at once if a
            // note of the right amount was there, else after the next block. A buy offer
            // with a deposit is listed once its bitcoin is there.
            let st = load(e.wallet);
            let mine = st.offers.iter().find(|m| m.offer.id == o.id);
            let listed = mine.is_none_or(|m| {
                m.reserve.as_ref().is_none_or(|r| r.posted)
                    && m.fund.as_ref().is_none_or(|f| f.posted)
            });
            let fund = mine.and_then(|m| m.fund.as_ref()).map(|f| {
                json!({"address": f.address, "need": f.need, "need_text": btc_text(f.need),
                       "fee_text": btc_text(f.need.saturating_sub(o.sats)), "back_to": to})
            });
            Ok(json!({"id": o.id, "listed": listed, "fund": fund}))
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct RecoverReq {
    /// Where the bitcoin found goes.
    address: String,
    #[serde(default)]
    password: String,
    /// How many deposit addresses to look at in every lane (3 if not given; at most 50).
    /// This copy's own lane is always looked through in full.
    #[serde(default)]
    count: Option<u32>,
}

/// Looks for bitcoin left at this wallet's deposit addresses for buy offers and sends it
/// to `address`. For a wallet restored from its recovery phrase, which has no record of
/// its offers: the addresses derive from the phrase. Deposits of offers that are open or
/// in a swap now are left alone.
pub async fn recover_btc(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<RecoverReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        with_engine(&a, false, |e, _| {
            engine::btc_address(&r.address, e.btc_network)?;
            let st = load(e.wallet);
            let in_use = deposits_in_use(e.wallet, &st);
            // This copy's own lane in full, then the first numbers of every lane (another
            // copy of the wallet, or the file this one was restored in place of).
            let per_lane = r.count.unwrap_or(3).clamp(1, 50);
            let own = st.lane.into_iter().flat_map(|l| {
                (0..st.deposits.saturating_add(3)).map(move |c| l + DEPOSIT_LANES * c)
            });
            let all =
                (0..per_lane).flat_map(|c| (0..DEPOSIT_LANES).map(move |l| l + DEPOSIT_LANES * c));
            let mut seen = std::collections::HashSet::new();
            let numbers: Vec<u32> = own
                .chain(all)
                .filter(|d| !in_use.contains(d) && seen.insert(*d))
                .collect();
            let (mut sent, mut errors) = (Vec::new(), Vec::new());
            for d in &numbers {
                match e.sweep_deposit(*d, &r.address) {
                    Ok(Some(id)) => sent.push(id.to_string()),
                    Ok(None) => {}
                    Err(x) => {
                        if errors.len() < 5 {
                            errors.push(format!("address {d}: {x}"));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(json!({"checked": numbers.len(), "sent": sent, "errors": errors}))
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct TakeReq {
    id: String,
    address: String,
    #[serde(default)]
    password: String,
    /// Buying: pay the bitcoin now to an address of this wallet; the swap then starts
    /// and completes by itself. Otherwise the wallet shows where to pay once the seller's
    /// GNR is locked.
    #[serde(default)]
    deposit: bool,
    /// The wallet's Bitcoin balance (1.0.52): buying pays from it at once, selling is paid
    /// into it, and change and any refund go back to it. `address` is not used then.
    #[serde(default)]
    balance: bool,
}

#[derive(Deserialize)]
pub struct UntakeReq {
    id: String,
}

/// Calls off a purchase that still waits for its bitcoin. What arrives at its address
/// anyway is sent back.
pub async fn untake(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<UntakeReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let mut g = a.wallet.lock();
        let w = g.as_mut().context("wallet locked")?;
        let mut st = load(w);
        let tk = st
            .takes
            .iter_mut()
            .find(|x| x.offer.id == r.id && !x.gone && x.swap_id.is_none())
            .context("this has started or is over; see Your swaps")?;
        tk.gone = true;
        tk.ended = now();
        tk.why = "Called off by you. Nothing was locked or paid.".into();
        let back = tk.btc_address.clone();
        if let Some(f) = tk.fund.as_mut() {
            f.closed = Some(now());
            f.last_check = 0;
            f.status = format!("Called off. What arrives at the address is sent back to {back}.");
        }
        save(w, &st)
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn take(axum::extract::State(app): S, h: HeaderMap, Json(r): Json<TakeReq>) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        with_engine(&a, true, |e, base| {
            if r.balance {
                take_with_balance(e, base, &r.id)
            } else {
                take_offer(e, base, &r.id, &r.address, r.deposit)
            }
        })
    })
    .await
    {
        Ok((id, fund)) => Json(json!({"id": id, "fund": fund.map(|f| json!({
            "address": f.address, "need": f.need, "need_text": btc_text(f.need), "take": true,
        }))}))
        .into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct CancelReq {
    id: String,
}

/// Takes this wallet's offer `m` off the board for good: down from the board if it is
/// there, its GNR released (committed for a week from publishing if it was listed), the
/// bitcoin of a buy offer's deposit sent back with `fund_status`.
fn take_down(w: &mut Wallet, base: &str, m: &mut MyOffer, fund_status: &str) -> Result<()> {
    // An offer still on its way to the board is not there to be taken down.
    let listed =
        m.reserve.as_ref().is_none_or(|r| r.posted) && m.fund.as_ref().is_none_or(|f| f.posted);
    if listed {
        post(
            base,
            "/v1/swap/offers/cancel",
            &json!({"id": m.offer.id, "secret": m.cancel_secret}),
        )?;
    }
    m.cancelled = true;
    if let Some(r) = m.reserve.as_mut()
        && !r.released
    {
        // The GNR of an offer that was on the board stays committed for a week from
        // when it was published; one that never got there is free at once.
        let until = commit_until(r, m.offer.expires, now());
        if let Some(cm) = r.note() {
            w.release(&cm, until)?;
        }
        r.released = true;
        r.free_at = until;
        r.status.clear();
    }
    // A buy offer with a deposit: the bitcoin goes back (the background round sends it
    // within seconds).
    close_fund(m, true, fund_status);
    Ok(())
}

pub async fn cancel(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<CancelReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let base = board_url(&a);
        let mut g = a.wallet.lock();
        let w = g.as_mut().context("wallet locked")?;
        let mut st = load(w);
        let m = st
            .offers
            .iter_mut()
            .find(|o| o.offer.id == r.id)
            .context("not your offer")?;
        ensure!(
            !m.taken,
            "this offer was already taken; the swap continues in Your swaps"
        );
        let back = m.btc_address.clone();
        take_down(
            w,
            &base,
            m,
            &format!("Cancelled. Sending your bitcoin back to {back}."),
        )?;
        save(w, &st)
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fol(limit: f64, margin: f64, trigger: Option<f64>) -> Follow {
        Follow {
            limit,
            at: 0,
            moves: 0,
            margin,
            trigger,
        }
    }

    #[test]
    fn a_following_offer_stands_at_its_margin_from_the_market_within_its_limit() {
        // Sell at the market +2 %, never below 4,000 sat per GNR.
        let f = fol(4_000.0, 0.02, None);
        assert!((follow_price(Side::SellGnr, &f, 5_000.0) - 5_100.0).abs() < 1e-9);
        assert_eq!(follow_price(Side::SellGnr, &f, 3_000.0), 4_000.0);
        // Buy at the market -1 %, never above 6,000.
        let b = fol(6_000.0, -0.01, None);
        assert!((follow_price(Side::BuyGnr, &b, 5_000.0) - 4_950.0).abs() < 1e-9);
        assert_eq!(follow_price(Side::BuyGnr, &b, 9_000.0), 6_000.0);
        // An offer saved before 1.0.48 reads with no margin and no trigger.
        let old: Follow =
            serde_json::from_value(json!({"limit": 4_000.0, "at": 5, "moves": 2})).unwrap();
        assert_eq!((old.margin, old.trigger), (0.0, None));
        // Moving toward the market and its margin, 5 % a step at most.
        let o = Offer {
            id: String::new(),
            network: "devnet".into(),
            btc_network: "regtest".into(),
            side: Side::SellGnr,
            gnr: 100_000_000,
            sats: 5_000,
            key: String::new(),
            expires: 0,
            cancel: String::new(),
            nonce: 0,
        };
        assert_eq!(
            follow_target(&o, &f, 5_000.0),
            None,
            "5,100 is within a step but 10,000 sat is the least a part needs"
        );
        let big = Offer {
            gnr: 100 * 100_000_000,
            sats: 500_000,
            ..o
        };
        assert_eq!(
            follow_target(&big, &f, 5_000.0),
            Some((100 * 100_000_000, 510_000))
        );
        assert_eq!(
            follow_target(&big, &f, 9_000.0),
            Some((100 * 100_000_000, 525_000)),
            "one step of 5 %"
        );
    }

    #[test]
    fn a_trigger_takes_the_offer_down_when_the_market_reaches_it() {
        let s = fol(1.0, 0.0, Some(4_000.0));
        assert!(!triggered(Side::SellGnr, &s, 4_500.0) && triggered(Side::SellGnr, &s, 4_000.0));
        let b = fol(1e9, 0.0, Some(6_000.0));
        assert!(!triggered(Side::BuyGnr, &b, 5_500.0) && triggered(Side::BuyGnr, &b, 6_100.0));
        assert!(!triggered(Side::SellGnr, &fol(1.0, 0.0, None), 1.0));
    }

    fn my(expires: u64) -> MyOffer {
        MyOffer {
            n: 1,
            offer: Offer {
                id: "00".repeat(16),
                network: "devnet".into(),
                btc_network: "regtest".into(),
                side: Side::SellGnr,
                gnr: 100_000_000,
                sats: 50_000,
                key: String::new(),
                expires,
                cancel: String::new(),
                nonce: 0,
            },
            cancel_secret: String::new(),
            btc_address: String::new(),
            taken: false,
            cancelled: false,
            republished: false,
            pending_sent: false,
            pending_cleared: false,
            pending_at: 0,
            reserve: Some(Reserve {
                posted: true,
                ..Default::default()
            }),
            funded_sent: false,
            fund: None,
            prepaid_only: false,
            follow: None,
            was: Vec::new(),
            auto: None,
            part: None,
            rest_deposit: None,
            rest_done: false,
            promise: None,
            stopped: None,
        }
    }

    #[test]
    fn a_quick_swap_offer_is_split_into_lots_at_one_price() {
        const G: u64 = 100_000_000;
        assert_eq!(lots_for(150_000), 1);
        assert_eq!(lots_for(500_000), 2);
        assert_eq!(lots_for(5_000_000), MAX_LOTS);
        for side in [Side::SellGnr, Side::BuyGnr] {
            for (gnr, sats, k) in [
                (10 * G + 7, 1_000_001, 3),
                (3 * G, 300_000, 2),
                (G, 10_000, 1),
            ] {
                let lots = split_lots(side, gnr, sats, k);
                assert_eq!(lots.len(), k);
                assert_eq!(
                    lots.iter().map(|x| x.0).sum::<u64>(),
                    gnr,
                    "the GNR is all there"
                );
                let paid: u64 = lots.iter().map(|x| x.1).sum();
                match side {
                    // A seller never asks less in all than the whole offer.
                    Side::SellGnr => assert!(paid >= sats && paid <= sats + k as u64),
                    // A buyer never pays more in all than the whole offer.
                    Side::BuyGnr => assert!(paid <= sats && paid + k as u64 >= sats),
                }
                // Every lot keeps the offer's price, to the sat.
                for (g, s) in lots {
                    let p = s as f64 / (g as f64 / 1e8);
                    let want = sats as f64 / (gnr as f64 / 1e8);
                    assert!((p - want).abs() / want < 1e-3, "{p} vs {want}");
                }
            }
        }
    }

    #[test]
    fn gnr_stays_set_aside_only_while_something_needs_it() {
        let t = 1_800_000_000;
        // An open offer keeps its GNR; one that ran its time gives it back.
        assert_eq!(held_for(Some(&my(t + 60)), false, t), Held::Keep);
        assert_eq!(held_for(Some(&my(t)), false, t), Held::Over);
        // The same while the offer is still on its way to the board.
        let mut m = my(t + 60);
        m.reserve = Some(Reserve::default());
        assert_eq!(held_for(Some(&m), false, t), Held::Keep);
        m.offer.expires = t - 1;
        assert_eq!(held_for(Some(&m), false, t), Held::Over);
        // Taken: its swap decides, however old the offer is.
        let mut m = my(t - 60);
        m.taken = true;
        assert_eq!(held_for(Some(&m), true, t), Held::Keep);
        assert_eq!(held_for(Some(&m), false, t), Held::Over);
        // What was listed is committed for a week from its publication, however the
        // offer ended; what never reached the board is not; older offers keep their rule.
        let r = |listed, posted| Reserve {
            listed,
            posted,
            ..Default::default()
        };
        assert_eq!(
            commit_until(&r(t - 3600, true), t + 60, t),
            Some(t - 3600 + COMMIT_SECS)
        );
        assert_eq!(
            commit_until(&r(t - 3600, false), t - 1, t),
            Some(t - 3600 + COMMIT_SECS)
        );
        assert_eq!(commit_until(&r(t - COMMIT_SECS, true), t + 60, t), None);
        assert_eq!(commit_until(&r(0, false), t + 60, t), None);
        assert_eq!(commit_until(&r(0, true), t + 3600, t), Some(t + 3600));
        assert_eq!(
            commit_until(&r(0, true), t + 30 * 24 * 3600, t),
            Some(t + CANCEL_HOLD_SECS)
        );
        assert_eq!(commit_until(&r(0, true), t - 1, t), None);
        // Cancelled or given up, already released, from before offers set GNR aside,
        // or unknown to this wallet: nothing is kept.
        let mut m = my(t + 60);
        m.cancelled = true;
        assert_eq!(held_for(Some(&m), true, t), Held::Free);
        let mut m = my(t + 60);
        m.reserve = Some(Reserve {
            released: true,
            ..Default::default()
        });
        assert_eq!(held_for(Some(&m), false, t), Held::Free);
        let mut m = my(t + 60);
        m.reserve = None;
        assert_eq!(held_for(Some(&m), false, t), Held::Free);
        assert_eq!(held_for(None, false, t), Held::Free);
    }
}
