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
}

/// Moves a taken offer to "in progress" on the board.
fn announce_pending(base: &str, m: &MyOffer) -> bool {
    post(
        base,
        "/v1/swap/progress",
        &json!({"offer": m.offer, "secret": m.cancel_secret}),
    )
    .is_ok()
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
) -> Result<Offer> {
    engine::btc_address(btc_address, e.btc_network)?;
    ensure!(sats >= 10_000, "offer at least 10,000 sat");
    ensure!(
        gnr > 2 * krypton_wallet_core::DEFAULT_FEE,
        "the GNR amount is too small"
    );
    if side == Side::SellGnr {
        let have = available(e.wallet);
        ensure!(
            have >= gnr + krypton_wallet_core::DEFAULT_FEE,
            "this wallet has {} GNR free (the rest is spoken for by your open sell offers)",
            format_amount(have)
        );
    }
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
        expires: now() + board::MAX_OFFER_SECS,
        cancel: board::cancel_hash(&secret),
        nonce: 0,
    };
    offer.seal_pow();
    post(base, "/v1/swap/offers", &offer)?;
    st.offers.push(MyOffer {
        n,
        offer: offer.clone(),
        cancel_secret: hex::encode(secret),
        btc_address: btc_address.trim().into(),
        taken: false,
        cancelled: false,
        republished: false,
        pending_sent: false,
        pending_cleared: false,
    });
    save(e.wallet, &st)?;
    Ok(offer)
}

/// GNR held back for this wallet's open sell offers (amount + fee each), so a buyer who
/// takes one always finds the coins there: sends and other offers cannot use them.
pub fn reserved(w: &Wallet) -> u64 {
    let t = now();
    load(w)
        .offers
        .iter()
        .filter(|m| {
            m.offer.side == Side::SellGnr && !m.taken && !m.cancelled && m.offer.expires > t
        })
        .map(|m| m.offer.gnr.saturating_add(krypton_wallet_core::DEFAULT_FEE))
        .fold(0u64, u64::saturating_add)
}

/// Spendable GNR not reserved for open sell offers.
pub fn available(w: &Wallet) -> u64 {
    w.balance().spendable.saturating_sub(reserved(w))
}

/// Tells the board this wallet is online for its open offers (at most every 45 s).
fn ping(base: &str, st: &State) {
    static LAST: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    let t = now();
    let open: Vec<&MyOffer> = st
        .offers
        .iter()
        .filter(|m| !m.taken && !m.cancelled && m.offer.expires > t)
        .collect();
    let Some(first) = open.first() else { return };
    let key = format!("{base}|{}", first.offer.id);
    let mut last = LAST.get_or_init(Default::default).lock();
    if last
        .get(&key)
        .is_some_and(|i| i.elapsed() < Duration::from_secs(45))
    {
        return;
    }
    let items: Vec<Value> = open
        .iter()
        .take(500)
        .map(|m| json!({"id": m.offer.id, "secret": m.cancel_secret}))
        .collect();
    if post(base, "/v1/swap/offers/ping", &json!({ "items": items })).is_ok() {
        last.insert(key, Instant::now());
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
    let offer: Offer = serde_json::from_value(raw)?;
    offer.validate(e.wallet.network().as_str(), now())?;
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
    ping(&base, &st);
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
        let fresh = matches!(s.stage, Stage::Requested | Stage::Accepted)
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
        let over = swaps
            .iter()
            .find(|s| s.id == l.swap_id)
            .is_none_or(|s| s.stage.finished());
        if over {
            let gone = post(
                &base,
                "/v1/swap/offers/cancel",
                &json!({"id": m.offer.id, "secret": m.cancel_secret}),
            );
            m.pending_cleared = gone.is_ok();
        } else if !m.pending_sent {
            m.pending_sent = announce_pending(&base, m);
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
            let Msg::Request { gnr, sats, .. } = msg else {
                bail!("unexpected code")
            };
            // Someone wants to buy our GNR offer.
            if let Some(mo) = st
                .offers
                .iter_mut()
                .find(|o| o.n == n && o.offer.id == offer && o.offer.side == Side::SellGnr)
            {
                if mo.taken || mo.cancelled {
                    return send_note(base, from, key, &Note::Gone { offer });
                }
                ensure!(
                    gnr == mo.offer.gnr && sats == mo.offer.sats,
                    "a request with other amounts than the offer"
                );
                let addr = mo.btc_address.clone();
                mo.taken = true;
                mo.pending_sent = announce_pending(base, mo);
                let s = e.accept(&code, &addr)?;
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
            mo.taken = true;
            mo.pending_sent = announce_pending(base, mo);
            let (gnr, sats, addr) = (mo.offer.gnr, mo.offer.sats, mo.btc_address.clone());
            let s = e.request(gnr, sats, &addr)?;
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
                v
            }).collect::<Vec<_>>(),
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
        let gnr = parse_amount(&r.gnr).context("invalid GNR amount")?;
        let sats = Amount::from_str_in(r.btc.trim(), Denomination::Bitcoin)
            .map_err(|e| anyhow!("invalid BTC amount: {e}"))?
            .to_sat();
        with_engine(&a, false, |e, base| {
            publish_offer(e, base, r.side, gnr, sats, &r.address)
        })
    })
    .await
    {
        Ok(o) => Json(json!({"id": o.id})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct TakeReq {
    id: String,
    address: String,
}

pub async fn take(axum::extract::State(app): S, h: HeaderMap, Json(r): Json<TakeReq>) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || with_engine(&a, true, |e, base| take_offer(e, base, &r.id, &r.address)))
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
        post(
            &base,
            "/v1/swap/offers/cancel",
            &json!({"id": m.offer.id, "secret": m.cancel_secret}),
        )?;
        m.cancelled = true;
        save(w, &st)
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}
