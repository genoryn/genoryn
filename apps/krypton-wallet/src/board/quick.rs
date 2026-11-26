//! Quick Swap (1.0.46) from the wallet's side.
//!
//! * **Taking**: buy or sell an amount of GNR at the best prices on the board in one step.
//!   The wallet spreads it over up to `board::MAX_PARTS` Quick Swap offers whose makers
//!   are online and whose coins are there, each part its own atomic swap. Buying pays from
//!   the wallet's Bitcoin balance at once; selling is paid into it. A part that does not
//!   start is replaced by another, never at a worse price than the one the user accepted.
//! * **Making**: a Quick Swap offer is swapped in parts by itself. After a part, what is
//!   left of the offer goes back on the board as soon as its coins are in a block again.
//! * **The Bitcoin balance**: one address of the wallet's own (`deposit::BALANCE`).

use super::*;
use std::collections::HashSet;

/// A Quick Swap offer on the board: as listed, as an offer, and its setting.
pub(super) struct Listed {
    raw: Value,
    offer: Offer,
    auto: board::Auto,
    /// Its price is beyond the price guard: Quick Swap never takes it.
    far: bool,
}

/// The Quick Swap price guard (1.0.48): a part is never more than this much worse for the
/// taker than the market price (dearer when buying GNR, cheaper when selling it), when the
/// price rests on enough verified swaps. Anyone can post an offer at any price; one click
/// must not buy at a price far from what GNR trades at.
pub const QUICK_GUARD: f64 = 0.20;
/// The guard while the price is still forming or the market is thin.
pub const QUICK_GUARD_THIN: f64 = 0.10;

/// The guard for the board's market `m` (`/v1/swap/market` → `market`).
pub(super) fn guard_band(m: &Value) -> f64 {
    if m["status"] == "verified" && m["depth"] != "thin" {
        QUICK_GUARD
    } else {
        QUICK_GUARD_THIN
    }
}

/// Whether the board says the swaps are running away from the published price: Quick Swap
/// waits until the price has caught up (the offer board works as usual meanwhile).
pub(super) fn paused(m: &Value) -> bool {
    m["volatile"].as_bool().unwrap_or(false)
}

/// The worst price (sat per GNR) Quick Swap takes from offers of `side`: the market price,
/// or without one the best of `prices` (that side's offers), `band` worse for the taker.
/// `None` when there is nothing to measure against.
pub(super) fn guard_limit(
    side: Side,
    market: Option<f64>,
    prices: &[f64],
    band: f64,
) -> Option<f64> {
    let ok = prices.iter().copied().filter(|p| p.is_finite() && *p > 0.0);
    let best = match side {
        Side::SellGnr => ok.min_by(f64::total_cmp),
        Side::BuyGnr => ok.max_by(f64::total_cmp),
    };
    let reference = market.filter(|m| m.is_finite() && *m > 0.0).or(best)?;
    Some(match side {
        Side::SellGnr => reference * (1.0 + band),
        Side::BuyGnr => reference * (1.0 - band),
    })
}

/// Whether an offer of `side` at `price` is within the guard `limit`.
pub(super) fn near(side: Side, price: f64, limit: Option<f64>) -> bool {
    limit.is_none_or(|l| match side {
        Side::SellGnr => price <= l * (1.0 + 1e-9),
        Side::BuyGnr => price >= l * (1.0 - 1e-9),
    })
}

/// Marks the offers of `book` beyond the price guard of their side.
fn mark_far(book: &mut [Listed], market: Option<f64>, band: f64) {
    for side in [Side::SellGnr, Side::BuyGnr] {
        let prices: Vec<f64> = book
            .iter()
            .filter(|l| l.offer.side == side)
            .map(|l| l.offer.price())
            .collect();
        let limit = guard_limit(side, market, &prices, band);
        for l in book.iter_mut().filter(|l| l.offer.side == side) {
            l.far = !near(side, l.offer.price(), limit);
        }
    }
}

/// The board's Quick Swap summary (`/v1/swap/market` → `quick`) with the price guard
/// applied: each side counts only the offers Quick Swap takes, and says how many it leaves
/// out. `offers` is the board's listing (`/v1/swap/offers` → `offers`). This wallet's own
/// offers (`mine`) are not counted (it cannot swap with itself) but said (`mine` per side).
pub(super) fn guard_summary(
    quick: &mut Value,
    offers: &[Value],
    market: Option<f64>,
    band: f64,
    mine: &[&String],
) {
    /// An offer as the summary counts it: GNR, sat, price.
    type Live = (u64, u64, f64);
    let own = |o: &Value| {
        o["id"]
            .as_str()
            .is_some_and(|id| mine.iter().any(|m| m.as_str() == id))
    };
    for (key, side) in [("sell", Side::SellGnr), ("buy", Side::BuyGnr)] {
        let of_side =
            |o: &&Value| serde_json::from_value::<Side>(o["side"].clone()).ok() == Some(side);
        let mine_n = offers
            .iter()
            .filter(of_side)
            .filter(|o| o["auto"].is_object() && own(o))
            .count();
        let live: Vec<Live> = offers
            .iter()
            .filter(|o| !own(o))
            .filter(|o| o["auto"].is_object() && o["online"].as_bool().unwrap_or(false))
            .filter(|o| serde_json::from_value::<Side>(o["side"].clone()).ok() == Some(side))
            .filter(|o| match side {
                Side::SellGnr => o["backed"].as_bool().unwrap_or(false),
                Side::BuyGnr => o["deposited"].as_bool().unwrap_or(false),
            })
            .filter_map(|o| {
                let (g, s) = (o["gnr"].as_u64()?, o["sats"].as_u64()?);
                (g > 0).then(|| (g, s, price_of(g, s)))
            })
            .collect();
        let prices: Vec<f64> = live.iter().map(|x| x.2).collect();
        let limit = guard_limit(side, market, &prices, band);
        let (ok, far): (Vec<Live>, Vec<Live>) =
            live.into_iter().partition(|x| near(side, x.2, limit));
        let best = match side {
            Side::SellGnr => ok.iter().map(|x| x.2).min_by(f64::total_cmp),
            Side::BuyGnr => ok.iter().map(|x| x.2).max_by(f64::total_cmp),
        };
        let s = &mut quick[key];
        if !s.is_object() {
            *s = json!({});
        }
        s["offers"] = json!(ok.len());
        s["gnr"] = json!(ok.iter().map(|x| x.0).sum::<u64>());
        s["sats"] = json!(ok.iter().map(|x| x.1).sum::<u64>());
        s["best"] = json!(best);
        s["mine"] = json!(mine_n);
        s["far_offers"] = json!(far.len());
        s["far_gnr"] = json!(far.iter().map(|x| x.0).sum::<u64>());
        s["guard"] = json!(limit);
    }
    quick["guard_pct"] = json!((band * 100.0).round());
}

/// The Quick Swap offers that can be swapped with right now: their makers are online,
/// their coins are there (sell offers: GNR proven and unmoved; buy offers: bitcoin found
/// at the deposit), they run long enough for a swap, and they are not this wallet's own
/// or already taken by it.
fn book(e: &Engine<'_, Multi>, base: &str, st: &State) -> Result<Vec<Listed>> {
    let v = get(base, "/v1/swap/offers")?;
    Ok(book_from(e, &v, st))
}

/// The best parts for `want` among `book`, leaving out the offers in `skip`.
fn plan_in(
    book: &[Listed],
    buying: bool,
    want: board::Want,
    skip: &[String],
    max: usize,
) -> Vec<board::Part> {
    let v: Vec<(Offer, board::Auto)> = book
        .iter()
        .filter(|l| !l.far && !skip.contains(&l.offer.id))
        .map(|l| (l.offer.clone(), l.auto.clone()))
        .collect();
    board::plan(&v, buying, want, max)
}

fn price_of(gnr: u64, sats: u64) -> f64 {
    if gnr == 0 {
        0.0
    } else {
        sats as f64 / (gnr as f64 / 1e8)
    }
}

/// Whether a part's price is within what the user accepted.
fn within(buying: bool, p: &board::Part, limit: f64) -> bool {
    let x = price_of(p.gnr, p.sats);
    if buying {
        x <= limit * (1.0 + 1e-9)
    } else {
        x >= limit * (1.0 - 1e-9)
    }
}

/// The Bitcoin balance: what is there, confirmed and in all (less what this wallet
/// spent itself moments ago), and what running Quick Swap purchases will still pay.
pub(super) struct Balance {
    pub confirmed: u64,
    pub total: u64,
    pub committed: u64,
}

fn committed(w: &Wallet, st: &State) -> u64 {
    let swaps = engine::load(w);
    st.takes
        .iter()
        .filter(|x| x.fund.as_ref().is_some_and(|f| f.d == deposit::BALANCE))
        .filter(|x| !x.gone)
        .filter(|x| match &x.swap_id {
            None => true,
            Some(id) => swaps
                .iter()
                .find(|s| s.id == *id)
                .is_some_and(|s| !s.stage.finished() && s.paid_tx.is_none() && !s.ended_unlocked()),
        })
        .map(|x| x.amounts().1)
        .fold(0, u64::saturating_add)
}

pub(super) fn balance(e: &Engine<'_, Multi>, st: &State) -> Result<Balance> {
    let coins = e.deposit_coins(deposit::BALANCE)?;
    Ok(Balance {
        confirmed: coins
            .iter()
            .filter(|c| c.height.is_some())
            .map(|c| c.value.to_sat())
            .sum(),
        total: coins.iter().map(|c| c.value.to_sat()).sum(),
        committed: committed(e.wallet, st),
    })
}

/// What a Quick Swap part will cost in network fees, roughly: the payment into the
/// contract (buying) or the claim of the bitcoin (selling), at today's rate.
pub(super) fn btc_fee(e: &Engine<'_, Multi>) -> u64 {
    e.btc
        .fee_rate()
        .ok()
        .and_then(|r| r.fee_vb(deposit::PAY_VSIZE))
        .map(|a| a.to_sat())
        .unwrap_or(deposit::MIN_FEE_RESERVE)
        .max(500)
}

/// Starts the parts of `plan` for the Quick Swap `q`. Returns how many started; a part
/// that cannot start is left out (and its offer not tried again).
fn start_parts(
    e: &mut Engine<'_, Multi>,
    base: &str,
    book: &[Listed],
    q: &mut QuickOrder,
    plan: &[board::Part],
) -> (usize, Vec<String>) {
    let bal = e.deposit_address(deposit::BALANCE).to_string();
    let (mut started, mut errors) = (0, Vec::new());
    for p in plan {
        q.tried.push(p.id.clone());
        let Some(l) = book.iter().find(|l| l.offer.id == p.id) else {
            continue;
        };
        let pay = if q.buying {
            TakePay::Balance
        } else {
            TakePay::Later
        };
        match take_listed(
            e,
            base,
            &l.raw,
            &bal,
            pay,
            Some((p.gnr, p.sats)),
            Some(q.id.clone()),
        ) {
            Ok(_) => {
                // `take_listed` wrote the take; its number is the newest take's.
                if let Some(x) = load(e.wallet)
                    .takes
                    .iter()
                    .rev()
                    .find(|x| x.offer.id == p.id)
                {
                    q.takes.push(x.n);
                }
                started += 1;
            }
            Err(x) => errors.push(x.to_string()),
        }
    }
    (started, errors)
}

/// How a part of a Quick Swap stands.
#[derive(PartialEq, Eq, Clone, Copy)]
enum PartState {
    /// Waiting for the maker's wallet to answer.
    Asking,
    /// Its swap runs.
    Running,
    Done,
    /// It never started, or ended before anything was locked: it is replaced.
    Failed,
    /// It ended after something was locked (refunded): not replaced, nothing lost.
    Refunded,
}

fn part_state(x: &MyTake, swaps: &[engine::Swap]) -> PartState {
    match &x.swap_id {
        None if x.gone => PartState::Failed,
        None => PartState::Asking,
        Some(id) => match swaps.iter().find(|s| s.id == *id) {
            None => PartState::Failed,
            Some(s) if s.stage == Stage::Done => PartState::Done,
            Some(s) if s.ended_unlocked() => PartState::Failed,
            Some(s) if s.stage.finished() => PartState::Refunded,
            Some(_) => PartState::Running,
        },
    }
}

/// What part of a Quick Swap's `want` a take stands for.
fn counts(want: board::Want, x: &MyTake) -> u64 {
    let (g, s) = x.amounts();
    match want {
        board::Want::Gnr(_) => g,
        board::Want::Sats(_) => s,
    }
}

fn want_of(w: board::Want) -> u64 {
    match w {
        board::Want::Gnr(g) | board::Want::Sats(g) => g,
    }
}

/// Carries Quick Swaps on: lets go of parts whose maker did not answer, replaces parts
/// that did not start, and says how each one stands.
pub(super) fn advance_quick(e: &mut Engine<'_, Multi>, base: &str, st: &mut State) {
    let t = now();
    if st.quick.iter().all(|q| q.done) {
        return;
    }
    // Parts whose maker did not answer in time: let go of (and the maker told so). A
    // purchase has its swap from the start, waiting for the seller's accept: it ends
    // (nothing was locked or paid) and so the part is replaced.
    let silent: Vec<String> = engine::load(e.wallet)
        .iter()
        .filter(|s| s.stage == Stage::Requested && s.cancellable())
        .map(|s| s.id.clone())
        .collect();
    for x in st.takes.iter_mut().filter(|x| {
        x.quick.is_some()
            && !x.gone
            && x.at + QUICK_ANSWER_SECS < t
            && x.swap_id.as_ref().is_none_or(|id| silent.contains(id))
    }) {
        x.gone = true;
        x.ended = t;
        x.why = "The other wallet did not answer within two minutes; another offer is tried. Nothing was locked or paid.".into();
        if let Some(id) = &x.swap_id {
            let _ = e.cancel(id, "The seller's wallet did not answer within two minutes; another offer is tried. Nothing was locked or paid.");
        }
        let key = key_of(e.wallet, x.n);
        let _ = send_note(
            base,
            &x.offer.key,
            &key,
            &Note::Gone {
                offer: x.offer.id.clone(),
            },
        );
    }
    let swaps = engine::load(e.wallet);
    let mut listing: Option<Vec<Listed>> = None;
    for qi in 0..st.quick.len() {
        if st.quick[qi].done {
            continue;
        }
        let q = st.quick[qi].clone();
        let (covered, active, done, all) = {
            let parts: Vec<(&MyTake, PartState)> = st
                .takes
                .iter()
                .filter(|x| q.takes.contains(&x.n))
                .map(|x| (x, part_state(x, &swaps)))
                .collect();
            let live = |s: PartState| {
                matches!(s, PartState::Asking | PartState::Running | PartState::Done)
            };
            (
                parts
                    .iter()
                    .filter(|(_, s)| live(*s))
                    .map(|(x, _)| counts(q.want, x))
                    .sum::<u64>(),
                parts
                    .iter()
                    .filter(|(_, s)| matches!(s, PartState::Asking | PartState::Running))
                    .count(),
                parts.iter().filter(|(_, s)| *s == PartState::Done).count(),
                parts.len(),
            )
        };
        let left = want_of(q.want).saturating_sub(covered);
        let filling = t < q.created + QUICK_FILL_SECS && q.tried.len() < 12;
        let mut status = String::new();
        // Replacement parts for what is not covered, at most every quarter minute.
        if left > 0 && filling && active < board::MAX_PARTS && q.last_plan + 15 <= t {
            st.quick[qi].last_plan = t;
            if listing.is_none() {
                listing = book(e, base, st).ok();
            }
            if let Some(b) = listing.as_ref() {
                let want = match q.want {
                    board::Want::Gnr(_) => board::Want::Gnr(left),
                    board::Want::Sats(_) => board::Want::Sats(left),
                };
                let mut plan = plan_in(b, q.buying, want, &q.tried, board::MAX_PARTS - active);
                plan.retain(|p| within(q.buying, p, q.limit));
                // Buying: only what the balance still covers, confirmed.
                if q.buying && !plan.is_empty() {
                    match balance(e, st) {
                        Ok(bal) => {
                            let fee = btc_fee(e);
                            let mut room = bal.confirmed.saturating_sub(bal.committed);
                            plan.retain(|p| {
                                let ok = room >= p.sats + fee;
                                if ok {
                                    room -= p.sats + fee;
                                }
                                ok
                            });
                            if plan.is_empty() {
                                status = "Your Bitcoin balance does not cover more right now (bitcoin that just arrived, or change of a payment, needs one confirmation).".into();
                            }
                        }
                        Err(_) => plan.clear(),
                    }
                }
                if !plan.is_empty() {
                    let mut q2 = st.quick[qi].clone();
                    save(e.wallet, st).ok();
                    let (n, _) = start_parts(e, base, b, &mut q2, &plan);
                    // `take_listed` wrote takes to the wallet: read them back.
                    let fresh = load(e.wallet);
                    st.takes = fresh.takes;
                    st.links = fresh.links;
                    st.deposits = fresh.deposits;
                    st.lane = fresh.lane;
                    st.quick[qi] = q2;
                    if n > 0 {
                        continue;
                    }
                }
            }
        }
        // Over once nothing runs and nothing more is to start.
        if active == 0 && (left == 0 || !filling) {
            st.quick[qi].done = true;
        }
        st.quick[qi].status = if !status.is_empty() {
            status
        } else if active > 0 {
            format!(
                "{active} of {all} part{} running, {done} done.",
                if all == 1 { "" } else { "s" },
            )
        } else if left == 0 {
            "Done.".into()
        } else if filling {
            "Looking for another offer at your price.".into()
        } else if covered == 0 {
            "No Quick Swap offer could be swapped with at your price. Nothing was locked or paid."
                .into()
        } else {
            "Done as far as the board allowed at your price.".into()
        };
    }
    let n = st.quick.len();
    if n > 50 {
        st.quick.drain(..n - 50);
    }
}

/// Lists again what is left of Quick Swap offers after a part of them was swapped: a sell
/// offer's rest once the lock's change is sent (it is published after the next block, as
/// any sell offer), a buy offer's rest once the payment left it at the offer's new
/// deposit address (published after its confirmation). What is too small for an offer
/// is let go of: GNR is free at once, bitcoin goes back to its owner's address.
pub(super) fn list_rests(e: &mut Engine<'_, Multi>, st: &mut State, swaps: &[engine::Swap]) {
    struct Rest {
        from: usize,
        gnr: u64,
        sats: u64,
        reserve: Option<Reserve>,
        hold: Option<krypton_core::Hash32>,
        fund: Option<Fund>,
        viable: bool,
    }
    let t = now();
    let mut rests = Vec::new();
    for i in 0..st.offers.len() {
        let m = &st.offers[i];
        let (Some(_), Some((g, _))) = (m.auto.as_ref(), m.part) else {
            continue;
        };
        if m.rest_done {
            continue;
        }
        let Some(l) = st.links.iter().find(|l| l.offer == m.offer.id) else {
            continue;
        };
        let Some(s) = swaps.iter().find(|s| s.id == l.swap_id) else {
            continue;
        };
        let (lot_g, lot_s) = (m.offer.gnr as u128, m.offer.sats as u128);
        match m.offer.side {
            Side::SellGnr => {
                if s.lock_cm.is_none() {
                    // Ended before its lock: the whole offer goes back (`poll`).
                    if s.stage.finished() {
                        st.offers[i].rest_done = true;
                    }
                    continue;
                }
                let memo = format!("swap {}: GNR locked", s.index);
                let Some(h) = e.wallet.history().iter().find(|h| h.memo == memo) else {
                    continue;
                };
                let (Some(cm), txid) = (h.change_cm.as_deref().and_then(h32), h.txid.clone())
                else {
                    continue;
                };
                let value = m
                    .reserve
                    .as_ref()
                    .and_then(Reserve::note)
                    .and_then(|n| e.wallet.note(&n).map(|x| x.value))
                    .unwrap_or(m.offer.gnr + board::RESERVE_FEE);
                let change = value.saturating_sub(g + krypton_wallet_core::DEFAULT_FEE);
                let gnr = change.saturating_sub(board::RESERVE_FEE);
                let sats = (gnr as u128 * lot_s).div_ceil(lot_g.max(1)) as u64;
                let listed = m.reserve.as_ref().map(|r| r.listed).unwrap_or(0);
                rests.push(Rest {
                    from: i,
                    gnr,
                    sats,
                    reserve: Some(Reserve {
                        cm: Some(hex::encode(cm)),
                        txid: Some(txid),
                        status: "Swapping a part of this offer; the rest is published again after the next block.".into(),
                        listed,
                        ..Default::default()
                    }),
                    hold: Some(cm),
                    fund: None,
                    viable: true,
                });
            }
            Side::BuyGnr => {
                let Some(d) = m.rest_deposit else {
                    st.offers[i].rest_done = true;
                    continue;
                };
                if s.paid_tx.is_none() {
                    if s.stage.finished() {
                        st.offers[i].rest_done = true;
                    }
                    continue;
                }
                let Ok(coins) = e.deposit_coins(d) else {
                    continue;
                };
                let have: u64 = coins.iter().map(|c| c.value.to_sat()).sum();
                if have == 0 {
                    // The payment's change is not visible yet.
                    continue;
                }
                let reserve = e
                    .btc
                    .fee_rate()
                    .map(deposit::fee_reserve)
                    .unwrap_or(deposit::MIN_FEE_RESERVE);
                let sats = have.saturating_sub(reserve);
                let gnr = (sats as u128 * lot_g).div_ceil(lot_s.max(1)) as u64;
                rests.push(Rest {
                    from: i,
                    gnr,
                    sats,
                    reserve: None,
                    hold: None,
                    fund: Some(Fund {
                        d,
                        address: e.deposit_address(d).to_string(),
                        need: have,
                        since: t,
                        status: "Swapping a part of this offer; the rest is published again once its bitcoin is confirmed.".into(),
                        ..Default::default()
                    }),
                    viable: true,
                });
            }
        }
    }
    for mut r in rests {
        let m = st.offers[r.from].clone();
        st.offers[r.from].rest_done = true;
        r.viable = r.sats >= board::MIN_PART_SATS
            && r.gnr > 2 * krypton_wallet_core::DEFAULT_FEE
            && m.offer.expires > t + 900;
        let n = fresh_n(st);
        let key = key_of(e.wallet, n);
        let secret: [u8; 32] = rand::random();
        let offer = Offer {
            id: hex::encode(rand::random::<[u8; 16]>()),
            gnr: r.gnr,
            sats: r.sats,
            key: hex::encode(key.public().to_bytes()),
            cancel: board::cancel_hash(&secret),
            nonce: 0,
            ..m.offer.clone()
        };
        let auto = m.auto.clone().map(|a| AutoCfg {
            min: a.min.min(r.gnr),
            group: a.group,
        });
        let mut rest = MyOffer {
            n,
            offer,
            cancel_secret: hex::encode(secret),
            btc_address: m.btc_address.clone(),
            taken: false,
            cancelled: !r.viable,
            republished: false,
            pending_sent: false,
            pending_cleared: false,
            pending_at: 0,
            reserve: None,
            funded_sent: false,
            fund: None,
            prepaid_only: m.prepaid_only,
            follow: m.follow.clone(),
            was: Vec::new(),
            auto,
            part: None,
            rest_deposit: None,
            rest_done: false,
            promise: None,
            stopped: None,
        };
        if r.viable {
            if let (Some(cm), Some(res)) = (r.hold, r.reserve.take()) {
                // The change is held for the rest of the offer from now on.
                let _ = e.wallet.hold(cm, &rest.offer.id);
                rest.reserve = Some(res);
            }
            rest.fund = r.fund.take();
        } else if let Some(mut f) = r.fund.take() {
            // Too little bitcoin for an offer of its own: it goes back to its owner.
            f.closed = Some(t);
            f.last_check = 0;
            f.status = "What was left of this offer was too little to publish again; it is sent back to you.".into();
            rest.fund = Some(f);
        }
        if r.viable || rest.fund.is_some() {
            st.offers.push(rest);
        }
    }
}

/// Takes promises back from the board once their swap is paid or over.
pub(super) fn end_promises(base: &str, st: &mut State, swaps: &[engine::Swap]) {
    let over = |id: &str| {
        swaps
            .iter()
            .find(|s| s.id == id)
            .is_none_or(|s| s.deposit.is_some() || s.paid_tx.is_some() || s.stage.finished())
    };
    for m in st.offers.iter_mut().filter(|m| m.promise.is_some()) {
        let Some(l) = st.links.iter().find(|l| l.offer == m.offer.id) else {
            continue;
        };
        if over(&l.swap_id) {
            release_promise(base, m.promise.as_ref());
            m.promise = None;
        }
    }
    for x in st.takes.iter_mut().filter(|x| x.promise.is_some()) {
        if x.swap_id.as_deref().is_none_or(over) {
            release_promise(base, x.promise.as_ref());
            x.promise = None;
        }
    }
}

/// A Quick Swap buy offer whose seller has not answered its request in time: the swap
/// ends (nothing was locked or paid) and the offer goes back on the board.
pub(super) fn part_answers(e: &mut Engine<'_, Multi>, st: &State, swaps: &[engine::Swap], t: u64) {
    for m in st
        .offers
        .iter()
        .filter(|m| m.taken && m.auto.is_some() && m.offer.side == Side::BuyGnr)
    {
        for l in st.links.iter().filter(|l| l.offer == m.offer.id) {
            if let Some(s) = swaps.iter().find(|s| s.id == l.swap_id)
                && s.stage == Stage::Requested
                && s.created + PART_ANSWER_SECS < t
            {
                let _ = e.cancel(
                    &s.id,
                    "The seller did not answer within three minutes; the offer is published again. Nothing was locked or paid.",
                );
            }
        }
    }
}

// HTTP API.

#[derive(Deserialize)]
pub struct QuoteReq {
    /// `buy` or `sell` (GNR).
    side: String,
    amount: String,
    /// `gnr`, `btc` or `usd`.
    #[serde(default)]
    unit: String,
}

fn want_from(r: &QuoteReq) -> Result<(bool, board::Want)> {
    let buying = match r.side.as_str() {
        "buy" => true,
        "sell" => false,
        _ => bail!("buy or sell"),
    };
    let a = r.amount.trim();
    let want = match r.unit.as_str() {
        "btc" => board::Want::Sats(
            Amount::from_str_in(a, Denomination::Bitcoin)
                .map_err(|e| anyhow!("invalid BTC amount: {e}"))?
                .to_sat(),
        ),
        "usd" => {
            let usd: f64 = a
                .replace(',', "")
                .parse()
                .map_err(|_| anyhow!("invalid dollar amount"))?;
            let rate = cached_market()["btc_usd"]
                .as_f64()
                .filter(|r| *r > 0.0)
                .context(
                    "the bitcoin price is not known right now; give the amount in GNR or BTC",
                )?;
            ensure!(usd.is_finite() && usd > 0.0, "invalid dollar amount");
            board::Want::Sats((usd / rate * 1e8).round() as u64)
        }
        _ => board::Want::Gnr(parse_amount(a).context("invalid GNR amount")?),
    };
    ensure!(want_of(want) > 0, "say how much");
    Ok((buying, want))
}

/// Everything about a plan the Quick Swap card shows.
fn quote_json(
    e: &Engine<'_, Multi>,
    st: &State,
    book: &[Listed],
    buying: bool,
    want: board::Want,
) -> Value {
    let plan = plan_in(book, buying, want, &[], board::MAX_PARTS);
    let (gnr, sats) = plan.iter().fold((0u64, 0u64), |(g, s), p| {
        (g.saturating_add(p.gnr), s.saturating_add(p.sats))
    });
    let worst = plan
        .iter()
        .map(|p| price_of(p.gnr, p.sats))
        .fold(None::<f64>, |a, x| {
            Some(match a {
                None => x,
                Some(a) if buying => a.max(x),
                Some(a) => a.min(x),
            })
        });
    let side = || {
        book.iter()
            .filter(|l| (l.offer.side == Side::SellGnr) == buying)
    };
    let (av_gnr, av_sats, av_n) = side()
        .filter(|l| !l.far)
        .fold((0u64, 0u64, 0usize), |(g, s, n), l| {
            (g + l.offer.gnr, s + l.offer.sats, n + 1)
        });
    let (far_gnr, far_n) = side()
        .filter(|l| l.far)
        .fold((0u64, 0usize), |(g, n), l| (g + l.offer.gnr, n + 1));
    let kind = if buying { Side::SellGnr } else { Side::BuyGnr };
    let prices: Vec<f64> = side().map(|l| l.offer.price()).collect();
    let fee = btc_fee(e);
    let fees = fee * plan.len() as u64;
    let m = cached_market()["market"].clone();
    let market = market_price(&m);
    let band = guard_band(&m);
    let guard = guard_limit(kind, market, &prices, band);
    let price = price_of(gnr, sats);
    let timing = &e.timing;
    let biggest = plan.iter().map(|p| p.sats).max().unwrap_or(0);
    let minutes = timing.gnr_confs_for(biggest) + 10 * timing.btc_confs_for(biggest) as u64 + 3;
    let (mut can, mut why) = (!plan.is_empty(), String::new());
    if paused(&m) {
        can = false;
        why = "Swaps are trading far from the market price right now, and the price is catching up. Quick Swap waits until it has; the offer board works as usual.".into();
    } else if plan.is_empty() {
        why = if av_n == 0 && far_n > 0 && buying {
            "Every Quick Swap offer online asks more above the market price than Quick Swap accepts, so none is bought in one click. Take an offer on the board if you accept its price.".into()
        } else if av_n == 0 && far_n > 0 {
            "Every Quick Swap offer online pays more below the market price than Quick Swap accepts, so none is sold in one click. Take an offer on the board if you accept its price.".into()
        } else if av_n == 0 {
            "No Quick Swap offer is online on this side right now.".into()
        } else {
            "The amount is smaller than the smallest part the offers take (at least 10,000 sat)."
                .into()
        };
    }
    let mut bal = json!(null);
    if buying {
        if let Ok(b) = balance(e, st) {
            let room = b.confirmed.saturating_sub(b.committed);
            if can && room < sats + fees {
                can = false;
                why = if b.total.saturating_sub(b.committed) >= sats + fees {
                    "Your Bitcoin balance covers this once it has one confirmation (about ten minutes).".into()
                } else {
                    format!(
                        "Your Bitcoin balance has {} BTC free; this needs {} BTC with its network fee. Add bitcoin to your balance first.",
                        btc_text(room),
                        btc_text(sats + fees)
                    )
                };
            }
            bal = json!({"confirmed": b.confirmed, "total": b.total, "committed": b.committed, "free": room, "free_text": btc_text(room)});
        }
    } else {
        let have = available(e.wallet).saturating_sub(selling_now(st));
        let need = gnr + plan.len() as u64 * krypton_wallet_core::DEFAULT_FEE;
        if can && have < need {
            can = false;
            why = format!(
                "This wallet has {} GNR free; this needs {} GNR with its fees.",
                format_amount(have),
                format_amount(need)
            );
        }
        bal = json!({"gnr_free": have, "gnr_free_text": format_amount(have)});
    }
    let short = gnr > 0
        && match want {
            board::Want::Gnr(g) => gnr < g,
            board::Want::Sats(s) => sats + 10_000 < s,
        };
    json!({
        "buying": buying,
        "parts": plan.iter().map(|p| json!({
            "id": p.id, "gnr": p.gnr, "sats": p.sats,
            "gnr_text": format_amount(p.gnr), "btc_text": btc_text(p.sats),
            "price": price_of(p.gnr, p.sats),
        })).collect::<Vec<_>>(),
        "gnr": gnr, "sats": sats,
        "gnr_text": format_amount(gnr), "btc_text": btc_text(sats),
        "price": price, "worst": worst,
        "limit": worst.map(|w| if buying { w * (1.0 + QUICK_SLIP) } else { w * (1.0 - QUICK_SLIP) }),
        "market": market,
        "vs_market": market.filter(|m| *m > 0.0 && price > 0.0).map(|m| (price / m - 1.0) * 100.0),
        "short": short,
        "available": {"offers": av_n, "gnr": av_gnr, "sats": av_sats, "gnr_text": format_amount(av_gnr), "btc_text": btc_text(av_sats)},
        "far": {"offers": far_n, "gnr": far_gnr, "gnr_text": format_amount(far_gnr)},
        "guard": {"pct": (band * 100.0).round(), "limit": guard},
        "paused": paused(&m),
        // More than half of what is there near the market price: the last parts cost more.
        "big": gnr > 0 && gnr * 2 > av_gnr,
        "fee": fees, "fee_text": btc_text(fees),
        "minutes": minutes,
        "can": can, "why": why,
        "balance": bal,
        "btc_usd": cached_market()["btc_usd"],
    })
}

pub async fn quote(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<QuoteReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let (buying, want) = want_from(&r)?;
        // The board is read before the wallet is: the wallet stays free meanwhile.
        let base = board_url(&a);
        let raw = get(&base, "/v1/swap/offers")?;
        with_engine(&a, false, |e, base| {
            let st = load(e.wallet);
            let b = book_from(e, &raw, &st);
            let _ = base;
            Ok(quote_json(e, &st, &b, buying, want))
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

/// [`book`] from a listing already fetched.
fn book_from(e: &Engine<'_, Multi>, v: &Value, st: &State) -> Vec<Listed> {
    let mine: HashSet<&str> = st.offers.iter().map(|m| m.offer.id.as_str()).collect();
    let taken: HashSet<&str> = st
        .takes
        .iter()
        .filter(|x| !x.gone)
        .map(|x| x.offer.id.as_str())
        .collect();
    let t = now();
    let net = e.wallet.network().as_str().to_string();
    let btc = e.btc_network.to_core_arg().to_string();
    let mut book: Vec<Listed> = v["offers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|raw| {
            let auto: board::Auto = serde_json::from_value(raw["auto"].clone()).ok()?;
            let offer: Offer = serde_json::from_value(raw.clone()).ok()?;
            let ready = match offer.side {
                Side::SellGnr => raw["backed"].as_bool().unwrap_or(false),
                Side::BuyGnr => raw["deposited"].as_bool().unwrap_or(false),
            };
            let ok = ready
                && raw["online"].as_bool().unwrap_or(false)
                && offer.btc_network == btc
                && offer.expires > t + 900
                && !mine.contains(offer.id.as_str())
                && !taken.contains(offer.id.as_str())
                && board::auto_ok(&offer, &auto).is_ok()
                && offer.validate(&net, t).is_ok();
            ok.then(|| Listed {
                raw: raw.clone(),
                offer,
                auto,
                far: false,
            })
        })
        .collect();
    let m = &cached_market()["market"];
    mark_far(&mut book, market_price(m), guard_band(m));
    book
}

#[derive(Deserialize)]
pub struct StartReq {
    #[serde(flatten)]
    q: QuoteReq,
    /// The worst price (sat per GNR) the user accepts for any part.
    limit: f64,
    #[serde(default)]
    password: String,
}

pub async fn start(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<StartReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        let (buying, want) = want_from(&r.q)?;
        ensure!(r.limit.is_finite() && r.limit > 0.0, "no price to swap at");
        ensure!(
            !paused(&cached_market()["market"]),
            "Swaps are trading far from the market price right now, and the price is catching up. Quick Swap waits until it has; the offer board works as usual."
        );
        with_engine(&a, true, |e, base| {
            let st = load(e.wallet);
            let b = book(e, base, &st)?;
            let mut plan = plan_in(&b, buying, want, &[], board::MAX_PARTS);
            ensure!(!plan.is_empty(), "no Quick Swap offer can be swapped with right now; look again in a moment");
            let n = plan.len();
            plan.retain(|p| within(buying, p, r.limit));
            ensure!(
                plan.len() == n,
                "prices moved since you looked: look at the new quote and swap again"
            );
            let (gnr, sats) = plan.iter().fold((0u64, 0u64), |(g, s), p| {
            (g.saturating_add(p.gnr), s.saturating_add(p.sats))
        });
            let fees = btc_fee(e) * plan.len() as u64;
            if buying {
                let bal = balance(e, &st)?;
                ensure!(
                    bal.confirmed.saturating_sub(bal.committed) >= sats + fees,
                    "your Bitcoin balance does not cover {} BTC and its network fee yet (bitcoin needs one confirmation first)",
                    btc_text(sats)
                );
            } else {
                let have = available(e.wallet).saturating_sub(selling_now(&st));
                ensure!(
                    have >= gnr + plan.len() as u64 * krypton_wallet_core::DEFAULT_FEE,
                    "this wallet has {} GNR free",
                    format_amount(have)
                );
            }
            let mut q = QuickOrder {
                id: hex::encode(rand::random::<[u8; 8]>()),
                buying,
                want,
                limit: r.limit,
                created: now(),
                takes: vec![],
                tried: vec![],
                status: "Asking the makers' wallets.".into(),
                done: false,
                last_plan: now(),
            };
            let (started, errors) = start_parts(e, base, &b, &mut q, &plan);
            let mut st = load(e.wallet);
            ensure!(
                started > 0,
                "the swap could not start: {}",
                errors.first().cloned().unwrap_or_default()
            );
            let id = q.id.clone();
            st.quick.push(q);
            save(e.wallet, &st)?;
            Ok(json!({"id": id, "parts": started, "gnr_text": format_amount(gnr), "btc_text": btc_text(sats)}))
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

/// The Quick Swaps for the Swap tab: each with its parts and how they stand.
pub(super) fn quick_view(w: &Wallet, st: &State) -> Vec<Value> {
    let swaps = engine::load(w);
    let t = now();
    st.quick
        .iter()
        .rev()
        .filter(|q| !q.done || q.created + 24 * 3600 > t)
        .take(10)
        .map(|q| {
            let parts: Vec<Value> = st
                .takes
                .iter()
                .filter(|x| q.takes.contains(&x.n))
                .map(|x| {
                    let (g, s) = x.amounts();
                    let sw = x.swap_id.as_ref().and_then(|id| swaps.iter().find(|s| s.id == *id));
                    let state = match part_state(x, &swaps) {
                        PartState::Asking => "asking",
                        PartState::Running => "running",
                        PartState::Done => "done",
                        PartState::Failed => "failed",
                        PartState::Refunded => "refunded",
                    };
                    json!({
                        "gnr_text": format_amount(g), "btc_text": btc_text(s),
                        "price": price_of(g, s), "state": state,
                        "stage": sw.map(|s| s.stage), "status": sw.map(|s| s.status.clone()).unwrap_or_else(|| x.why.clone()),
                        "swap": x.swap_id, "since": x.at,
                    })
                })
                .collect();
            let (g, s) = st
                .takes
                .iter()
                .filter(|x| q.takes.contains(&x.n))
                .filter(|x| part_state(x, &swaps) == PartState::Done)
                .map(MyTake::amounts)
                .fold((0u64, 0u64), |(a, b), (g, s)| (a + g, b + s));
            json!({
                "id": q.id, "buying": q.buying, "created": q.created, "done": q.done,
                "want": q.want, "status": q.status, "parts": parts,
                "done_gnr_text": format_amount(g), "done_btc_text": btc_text(s),
            })
        })
        .collect()
}

/// Whether a Quick Swap is under way (the background round then runs often).
pub(super) fn busy(st: &State) -> bool {
    st.quick.iter().any(|q| !q.done)
}

// The Bitcoin balance.

pub async fn btc_info(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        with_engine(&a, false, |e, _| {
            let st = load(e.wallet);
            let b = balance(e, &st)?;
            let address = e.deposit_address(deposit::BALANCE).to_string();
            Ok(json!({
                "address": address,
                "confirmed": b.confirmed, "total": b.total, "committed": b.committed,
                "pending": b.total.saturating_sub(b.confirmed),
                "free": b.total.saturating_sub(b.committed),
                "confirmed_text": btc_text(b.confirmed), "total_text": btc_text(b.total),
                "pending_text": btc_text(b.total.saturating_sub(b.confirmed)),
                "committed_text": btc_text(b.committed),
                "free_text": btc_text(b.total.saturating_sub(b.committed)),
                "btc_usd": cached_market()["btc_usd"],
                "network": e.btc_network.to_core_arg(),
            }))
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct SendReq {
    address: String,
    /// BTC, or `max`.
    amount: String,
    #[serde(default)]
    password: String,
    /// Only check it and say what it would send, pay and leave (nothing is sent).
    #[serde(default)]
    preview: bool,
}

/// Sends bitcoin from the balance to an address of the user's choice. What running Quick
/// Swap purchases will still pay stays.
pub async fn btc_send(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<SendReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        if !r.preview {
            crate::confirm_password(&a, &r.password)?;
        }
        with_engine(&a, false, |e, _| {
            let dest = engine::btc_address(&r.address, e.btc_network)?;
            ensure!(
                dest != e.deposit_address(deposit::BALANCE),
                "that is this balance's own address"
            );
            let st = load(e.wallet);
            let b = balance(e, &st)?;
            let free = b.total.saturating_sub(b.committed);
            ensure!(free > 0, "there is no bitcoin free in the balance");
            if r.preview {
                // What the payment would be, with the fee as estimated now (the coins it
                // spends set the exact fee; it is never more than a tenth of the payment).
                let fee = btc_fee(e);
                let max = r.amount.trim().eq_ignore_ascii_case("max");
                let amount = if max {
                    let keep = if b.committed == 0 {
                        fee
                    } else {
                        b.committed + fee * 2
                    };
                    b.total.saturating_sub(keep)
                } else {
                    Amount::from_str_in(r.amount.trim(), Denomination::Bitcoin)
                        .map_err(|e| anyhow!("invalid BTC amount: {e}"))?
                        .to_sat()
                };
                ensure!(amount >= 1_000, "send at least 0.00001 BTC");
                ensure!(
                    amount + fee <= free,
                    "the balance has {} BTC free (running Quick Swaps still pay {} BTC)",
                    btc_text(free),
                    btc_text(b.committed)
                );
                return Ok(json!({
                    "address": dest.to_string(),
                    "amount": amount,
                    "fee": fee,
                    "total": amount + fee,
                    "free": free,
                    "left": free.saturating_sub(amount + fee),
                    "max": max,
                    "network": e.btc_network.to_core_arg(),
                }));
            }
            let id = if r.amount.trim().eq_ignore_ascii_case("max") {
                if b.committed == 0 {
                    e.sweep_deposit(deposit::BALANCE, &dest.to_string())?
                        .context("there is no bitcoin in the balance")?
                } else {
                    let keep = b.committed + btc_fee(e) * 2;
                    let amount = b.total.saturating_sub(keep);
                    ensure!(amount >= 1_000, "too little is free to send");
                    e.pay_from(deposit::BALANCE, &[(dest, amount)], deposit::BALANCE)?
                }
            } else {
                let amount = Amount::from_str_in(r.amount.trim(), Denomination::Bitcoin)
                    .map_err(|e| anyhow!("invalid BTC amount: {e}"))?
                    .to_sat();
                ensure!(amount >= 1_000, "send at least 0.00001 BTC");
                ensure!(
                    amount + btc_fee(e) <= free,
                    "the balance has {} BTC free (running Quick Swaps still pay {} BTC)",
                    btc_text(free),
                    btc_text(b.committed)
                );
                e.pay_from(deposit::BALANCE, &[(dest, amount)], deposit::BALANCE)?
            };
            Ok(json!({"txid": id.to_string()}))
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn the_guard_is_a_band_around_the_market_worse_for_the_taker() {
        assert_eq!(
            guard_limit(Side::SellGnr, Some(100.0), &[500.0], 0.2),
            Some(120.0)
        );
        assert_eq!(
            guard_limit(Side::BuyGnr, Some(100.0), &[10.0], 0.2),
            Some(80.0)
        );
        // Without a market price: the best offer of the side.
        assert_eq!(
            guard_limit(Side::SellGnr, None, &[150.0, 140.0, 9e9], 0.1),
            Some(154.0)
        );
        assert_eq!(
            guard_limit(Side::BuyGnr, None, &[90.0, 100.0, 1.0], 0.1),
            Some(90.0)
        );
        assert_eq!(guard_limit(Side::SellGnr, None, &[], 0.2), None);
        assert_eq!(guard_limit(Side::SellGnr, Some(f64::NAN), &[], 0.2), None);
        assert!(
            near(Side::SellGnr, 120.0, Some(120.0)) && !near(Side::SellGnr, 120.1, Some(120.0))
        );
        assert!(near(Side::BuyGnr, 80.0, Some(80.0)) && !near(Side::BuyGnr, 79.9, Some(80.0)));
        assert!(near(Side::SellGnr, 1e12, None));
        // 20 % once the price rests on enough verified swaps; 10 % while it forms or is thin.
        assert_eq!(
            guard_band(&json!({"status": "verified", "depth": "deep"})),
            0.2
        );
        assert_eq!(
            guard_band(&json!({"status": "verified", "depth": "moderate"})),
            0.2
        );
        assert_eq!(
            guard_band(&json!({"status": "verified", "depth": "thin"})),
            0.1
        );
        assert_eq!(
            guard_band(&json!({"status": "forming", "depth": "deep"})),
            0.1
        );
        assert_eq!(guard_band(&json!({})), 0.1, "a board before 1.0.48");
        assert!(paused(&json!({"volatile": true})) && !paused(&json!({})));
    }

    fn offer(side: &str, gnr: f64, btc: f64, quick: bool, online: bool) -> Value {
        json!({
            "side": side, "gnr": (gnr * 1e8).round() as u64, "sats": (btc * 1e8).round() as u64,
            "auto": if quick { json!({"min": 100_000}) } else { Value::Null },
            "online": online, "backed": side == "sell_gnr", "deposited": side == "buy_gnr",
        })
    }

    /// The board of 21 November 2026: 201 GNR of Quick Swap sell offers, of which 100 GNR
    /// at the market price and the rest at 400 and 68,000 times it.
    #[test]
    fn far_offers_are_not_counted_as_ready() {
        let mut offers = vec![
            offer("sell_gnr", 100.0, 0.00014681, true, true),
            offer("sell_gnr", 100.0, 0.000137, false, true),
            offer("sell_gnr", 50.0, 0.0095, true, false),
        ];
        for _ in 0..3 {
            offers.push(offer("sell_gnr", 33.333_333_33, 0.02, true, true));
            offers.push(offer("sell_gnr", 0.333_333_33, 0.033_333_34, true, true));
        }
        let mut q = json!({"sell": {"offers": 7, "gnr": 20_100_000_000u64}, "buy": {"offers": 0}});
        guard_summary(&mut q, &offers, Some(146.81), 0.1, &[]);
        assert_eq!(q["sell"]["offers"], 1);
        assert_eq!(q["sell"]["gnr"], 10_000_000_000u64);
        assert_eq!(q["sell"]["far_offers"], 6);
        assert_eq!(q["sell"]["best"].as_f64().map(|p| p.round()), Some(147.0));
        assert_eq!(q["buy"]["offers"], 0);
        assert_eq!(q["guard_pct"], 10.0);
        // This wallet's own offer is not counted (it cannot swap with itself), only said.
        let mut with_mine = offers.clone();
        let mut own = offer("buy_gnr", 100.0, 0.000127, true, true);
        own["id"] = json!("ab12");
        with_mine.push(own);
        let id = "ab12".to_string();
        let mut m = json!({"sell": {}, "buy": {}});
        guard_summary(&mut m, &with_mine, Some(127.0), 0.1, &[&id]);
        assert_eq!(
            (m["buy"]["offers"].clone(), m["buy"]["mine"].clone()),
            (json!(0), json!(1))
        );
        guard_summary(&mut m, &with_mine, Some(127.0), 0.1, &[]);
        assert_eq!(
            (m["buy"]["offers"].clone(), m["buy"]["mine"].clone()),
            (json!(1), json!(0))
        );
        // Without a market price the best Quick Swap offer is the measure: the same result.
        guard_summary(&mut q, &offers, None, 0.2, &[]);
        assert_eq!(
            (q["sell"]["offers"].clone(), q["sell"]["far_offers"].clone()),
            (json!(1), json!(6))
        );
    }
}
