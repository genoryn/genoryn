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
use krypton_swap::board::{self, Note, Offer, Side, Trade, offer_key};
use krypton_swap::engine::{self, Engine, Msg, Role, Stage};
use krypton_wallet_core::Wallet;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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
}

/// GNR of a cancelled sell offer can be sent again this long after the cancellation (or
/// when the offer would have ended, if that is sooner). It can go into a new offer at
/// once. This is a rule of the wallet, kept in its own file: the chain does not know it.
pub const CANCEL_HOLD_SECS: u64 = 24 * 3600;
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
            Held::RanOut => true,
        };
        if over && let Some(r) = m.as_mut().and_then(|m| m.reserve.as_mut()) {
            r.released = true;
            r.status = "The offer ran its time. Its GNR is free again.".into();
            changed = true;
        }
        let _ = w.release(&h.cm, None);
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
    /// Nothing needs the coins any more.
    Free,
    /// The offer ran its time without being taken.
    RanOut,
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
                Held::Free
            }
        }
        Some(m) if m.offer.expires <= t => Held::RanOut,
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
    post(
        base,
        "/v1/swap/progress",
        &json!({"id": m.offer.id, "offer": m.offer, "secret": m.cancel_secret, "funded": funded}),
    )
    .map(|_| ())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MyTake {
    n: u32,
    offer: Offer,
    btc_address: String,
    swap_id: Option<String>,
    gone: bool,
}

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
    offers: Vec<MyOffer>,
    takes: Vec<MyTake>,
    links: Vec<Link>,
    seen: Vec<String>,
    reported: Vec<String>,
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

fn publish_offer(
    e: &mut Engine<'_, Multi>,
    base: &str,
    side: Side,
    gnr: u64,
    sats: u64,
    btc_address: &str,
    secs: u64,
) -> Result<Offer> {
    engine::btc_address(btc_address, e.btc_network)?;
    let secs = secs.clamp(15 * 60, board::MAX_OFFER_SECS);
    ensure!(sats >= 10_000, "offer at least 10,000 sat");
    ensure!(
        gnr > 2 * krypton_wallet_core::DEFAULT_FEE,
        "the GNR amount is too small"
    );
    let mut st = load(e.wallet);
    st.next += 1;
    let n = st.next;
    let key = offer_key(e.wallet.swap_seed(), 1_000_000 + n);
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
    } else {
        offer.seal_pow();
        post(base, "/v1/swap/offers", &offer)?;
        mine.offer = offer.clone();
    }
    st.offers.push(mine);
    save(e.wallet, &st)?;
    Ok(offer)
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

/// Ends an offer that could not be listed: its GNR is free at once.
fn give_up(w: &mut Wallet, m: &mut MyOffer, why: String) {
    m.cancelled = true;
    if let Some(r) = m.reserve.as_mut() {
        if let Some(cm) = r.note() {
            let _ = w.release(&cm, None);
        }
        r.released = true;
        r.status = why;
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
        post(base, "/v1/swap/offers", &body)?;
        Ok(true)
    };
    match step(e, m, &mut r) {
        Ok(true) => {
            r.posted = true;
            r.tries = 0;
            r.status.clear();
        }
        Ok(false) => {}
        Err(x) => {
            let msg = x.to_string();
            if msg.contains("an offer with this id exists") {
                // It is on the board already.
                r.posted = true;
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
    offer_key(w.swap_seed(), 1_000_000 + n)
}

fn take_offer(
    e: &mut Engine<'_, Multi>,
    base: &str,
    id: &str,
    btc_address: &str,
) -> Result<String> {
    let raw = get(base, "/v1/swap/offers")?["offers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|o| o["id"].as_str() == Some(id))
        .context("this offer is no longer on the board")?;
    // Boards before 1.0.20 do not report it: treat as online.
    ensure!(
        raw["online"].as_bool().unwrap_or(true),
        "the maker of this offer is offline right now, so the swap could not start at once; pick an online offer"
    );
    let proven = raw["backed"].as_bool().unwrap_or(false);
    let offer: Offer = serde_json::from_value(raw)?;
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
    let mut st = load(e.wallet);
    ensure!(
        !st.offers.iter().any(|m| m.offer.id == id),
        "this is your own offer"
    );
    ensure!(
        !st.takes.iter().any(|t| t.offer.id == id && !t.gone),
        "you already took this offer"
    );
    engine::btc_address(btc_address, e.btc_network)?;
    st.next += 1;
    let n = st.next;
    let key = key_of(e.wallet, n);
    let mut take = MyTake {
        n,
        offer: offer.clone(),
        btc_address: btc_address.trim().into(),
        swap_id: None,
        gone: false,
    };
    match offer.side {
        // The maker sells GNR: we buy, so we start the swap with a request.
        Side::SellGnr => {
            let s = e.request(offer.gnr, offer.sats, btc_address)?;
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
            let have = available(e.wallet);
            ensure!(
                have >= offer.gnr + krypton_wallet_core::DEFAULT_FEE,
                "this wallet has {} GNR free (the rest is spoken for by your open sell offers)",
                format_amount(have)
            );
            send_note(
                base,
                &offer.key,
                &key,
                &Note::Take {
                    offer: offer.id.clone(),
                },
            )?;
        }
    }
    st.takes.push(take);
    save(e.wallet, &st)?;
    Ok(offer.id)
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
    // Sell offers on their way to the board: gather, prove, post.
    for m in st.offers.iter_mut() {
        advance(e, &base, m);
    }
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
        } else {
            m.cancelled = true;
        }
    }
    // Sell offers that ran their time: their GNR is free at once.
    for m in st.offers.iter_mut().filter(|m| {
        !m.taken
            && !m.cancelled
            && m.offer.expires <= t
            && m.reserve.as_ref().is_some_and(|r| !r.released)
    }) {
        if let Some(r) = m.reserve.as_mut() {
            if let Some(cm) = r.note() {
                let _ = e.wallet.release(&cm, None);
            }
            r.released = true;
            r.status = "The offer ran its time. Its GNR is free again.".into();
        }
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
    // Completed swaps count for the market price (amounts and hash lock only).
    let share = a.settings.lock().share_prices.unwrap_or(true);
    for s in swaps.iter().filter(|s| s.stage == Stage::Done) {
        let h = hex::encode(s.hash);
        if share && !st.reported.contains(&h) {
            let mut tr = Trade {
                network: e.wallet.network().as_str().into(),
                hash: h.clone(),
                gnr: s.gnr,
                sats: s.sats,
                nonce: 0,
            };
            tr.seal_pow();
            if post(&base, "/v1/swap/trade", &tr).is_ok() {
                st.reported.push(h);
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
            // Its GNR was never locked: the note is still there. Free it, so that the
            // new offer (below) takes exactly it.
            if let Some(r) = m.reserve.as_mut()
                && !r.released
            {
                if let Some(cm) = r.note() {
                    let _ = e.wallet.release(&cm, None);
                }
                r.released = true;
            }
            if m.offer.expires > t + 900 {
                again.push(m.clone());
            }
        }
    }
    let n = st.seen.len();
    if n > 1000 {
        st.seen.drain(..n - 1000);
    }
    save(e.wallet, &st)?;
    for m in again {
        let _ = publish_offer(
            e,
            &base,
            m.offer.side,
            m.offer.gnr,
            m.offer.sats,
            &m.btc_address,
            m.offer.expires.saturating_sub(now()),
        );
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
            // A swap already linked to this sender: its next code.
            if st.links.iter().any(|l| l.n == n && l.peer == from) {
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
                        && (s.gnr, s.sats) == (mo.offer.gnr, mo.offer.sats)
                        && s.from_note == note.map(hex::encode)
                        && !links.iter().any(|l| l.swap_id == s.id)
                });
                let s = match known {
                    Some(s) => s,
                    None => {
                        if mo.taken || mo.cancelled {
                            return send_note(base, from, key, &Note::Gone { offer });
                        }
                        ensure!(
                            gnr == mo.offer.gnr && sats == mo.offer.sats,
                            "a request with other amounts than the offer"
                        );
                        // An offer still on its way to the board cannot have been taken.
                        ensure!(
                            mo.reserve.as_ref().is_none_or(|r| r.posted),
                            "a request for an offer that is not published yet"
                        );
                        // The lock spends the note set aside for this offer, nothing
                        // else. The offer counts as taken only once the swap exists.
                        match e.accept_from(&code, &mo.btc_address, note) {
                            Ok(s) => s,
                            Err(x) => {
                                let _ = send_note(base, from, key, &Note::Gone { offer });
                                return Err(x);
                            }
                        }
                    }
                };
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
                    && x.offer.side == Side::BuyGnr
            }) {
                ensure!(
                    from == tk.offer.key,
                    "a request from someone other than the maker"
                );
                ensure!(
                    gnr == tk.offer.gnr && sats == tk.offer.sats,
                    "a request with other amounts than the offer"
                );
                let addr = tk.btc_address.clone();
                let s = e.accept(&code, &addr)?;
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
            bail!("a code for an offer this wallet does not know")
        }
        Note::Take { offer } => {
            let Some(mo) = st
                .offers
                .iter_mut()
                .find(|o| o.n == n && o.offer.id == offer && o.offer.side == Side::BuyGnr)
            else {
                bail!("a take for an unknown offer");
            };
            if mo.taken || mo.cancelled {
                return send_note(base, from, key, &Note::Gone { offer });
            }
            // The offer counts as taken only once its swap exists.
            let s = match e.request(mo.offer.gnr, mo.offer.sats, &mo.btc_address) {
                Ok(s) => s,
                Err(x) => {
                    let _ = send_note(base, from, key, &Note::Gone { offer });
                    return Err(x);
                }
            };
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

/// Swaps whose codes travel through the board.
pub fn linked_ids(w: &Wallet) -> std::collections::HashSet<String> {
    load(w).links.into_iter().map(|l| l.swap_id).collect()
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
    n > 0 && n <= 20
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
        let offers: Vec<Offer> = serde_json::from_value(raw["offers"].clone()).unwrap_or_default();
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
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut p| {
                let (g, s) = (
                    p["gnr"].as_u64().unwrap_or(0),
                    p["sats"].as_u64().unwrap_or(0),
                );
                p["gnr_text"] = json!(format_amount(g));
                p["btc_text"] = json!(Amount::from_sat(s).to_string_in(Denomination::Bitcoin));
                p["price"] = json!(if g > 0 {
                    s as f64 / (g as f64 / 1e8)
                } else {
                    0.0
                });
                p["age"] = json!(t0.saturating_sub(p["since"].as_u64().unwrap_or(t0)));
                p
            })
            .collect();
        let st = a.wallet.lock().as_ref().map(load).unwrap_or_default();
        let mine: Vec<&String> = st.offers.iter().map(|o| &o.offer.id).collect();
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
            })
        };
        // Fresh from the board (an offer just published shows at once), with the cached
        // BTC price.
        let mut market = get(&base, "/v1/swap/market").unwrap_or_else(|_| cached_market());
        market["btc_usd"] = cached_market()["btc_usd"].clone();
        Ok(json!({
            "board": base,
            "offers": offers.iter().map(view).collect::<Vec<_>>(),
            "in_progress": in_progress,
            "my_offers": st.offers.iter().rev().take(200).map(|m| {
                let mut v = view(&m.offer);
                v["taken"] = json!(m.taken); v["cancelled"] = json!(m.cancelled);
                v["expired"] = json!(m.offer.expires <= t);
                if let Some(r) = &m.reserve {
                    // On its way to the board: gathering the GNR, or proving it.
                    v["preparing"] = json!(!r.posted && !m.cancelled && !m.taken && m.offer.expires > t);
                    v["status"] = json!(r.status);
                    v["free_at"] = json!(r.free_at.filter(|u| *u > t));
                }
                v
            }).collect::<Vec<_>>(),
            "hold_secs": CANCEL_HOLD_SECS,
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
        let gnr = parse_amount(&r.gnr).context("invalid GNR amount")?;
        let sats = Amount::from_str_in(r.btc.trim(), Denomination::Bitcoin)
            .map_err(|e| anyhow!("invalid BTC amount: {e}"))?
            .to_sat();
        with_engine(&a, false, |e, base| {
            let o = publish_offer(
                e,
                base,
                r.side,
                gnr,
                sats,
                &r.address,
                r.hours.unwrap_or(24).clamp(1, board::MAX_OFFER_SECS / 3600) * 3600,
            )?;
            // A sell offer is listed once its GNR is set aside and proven: at once if a
            // note of the right amount was there, else after the next block.
            let listed = load(e.wallet)
                .offers
                .iter()
                .find(|m| m.offer.id == o.id)
                .is_none_or(|m| m.reserve.as_ref().is_none_or(|r| r.posted));
            Ok((o, listed))
        })
    })
    .await
    {
        Ok((o, listed)) => Json(json!({"id": o.id, "listed": listed})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct TakeReq {
    id: String,
    address: String,
    #[serde(default)]
    password: String,
}

pub async fn take(axum::extract::State(app): S, h: HeaderMap, Json(r): Json<TakeReq>) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        with_engine(&a, true, |e, base| take_offer(e, base, &r.id, &r.address))
    })
    .await
    {
        Ok(id) => Json(json!({"id": id})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct CancelReq {
    id: String,
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
        // An offer still on its way to the board is not there to be taken down.
        let listed = m.reserve.as_ref().is_none_or(|r| r.posted);
        if listed {
            post(
                &base,
                "/v1/swap/offers/cancel",
                &json!({"id": m.offer.id, "secret": m.cancel_secret}),
            )?;
        }
        m.cancelled = true;
        if let Some(r) = m.reserve.as_mut()
            && !r.released
        {
            // The GNR of an offer that was on the board is free again a day later, or
            // when the offer would have ended; one that never got there, at once.
            let until = listed.then(|| (now() + CANCEL_HOLD_SECS).min(m.offer.expires));
            if let Some(cm) = r.note() {
                w.release(&cm, until)?;
            }
            r.released = true;
            r.free_at = until.filter(|u| *u > now());
            r.status.clear();
        }
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
        }
    }

    #[test]
    fn gnr_stays_set_aside_only_while_something_needs_it() {
        let t = 1_800_000_000;
        // An open offer keeps its GNR; one that ran its time gives it back.
        assert_eq!(held_for(Some(&my(t + 60)), false, t), Held::Keep);
        assert_eq!(held_for(Some(&my(t)), false, t), Held::RanOut);
        // The same while the offer is still on its way to the board.
        let mut m = my(t + 60);
        m.reserve = Some(Reserve::default());
        assert_eq!(held_for(Some(&m), false, t), Held::Keep);
        m.offer.expires = t - 1;
        assert_eq!(held_for(Some(&m), false, t), Held::RanOut);
        // Taken: its swap decides, however old the offer is.
        let mut m = my(t - 60);
        m.taken = true;
        assert_eq!(held_for(Some(&m), true, t), Held::Keep);
        assert_eq!(held_for(Some(&m), false, t), Held::Free);
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
