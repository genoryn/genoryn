//! Krypton coins in the desktop wallet: balances, sends, the issuer's actions and the
//! coin market (one-transaction GNR ⇄ coin swaps through the board's sealed mailboxes).

use crate::{App, Multi, S, blocking, check_open, confirm_password, err, fee_options};
use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::Json;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use krypton_assets::{format_units, parse_units};
use krypton_core::address::Address;
use krypton_core::asset::{AssetId, Backing, Manifest, Supply};
use krypton_core::emission::{format_amount, parse_amount};
use krypton_swap::board::{self, CoinOffer, CoinTrade, Note, Opening, Side, offer_key};
use krypton_wallet_core::Wallet;
use krypton_wallet_core::coin_swap::{GnrHalf, SwapReceipt, SwapTerms};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const STORE_KEY: &str = "coin_board";
/// Coin-offer keys are numbered apart from bitcoin-offer keys.
const KEY_BASE: u32 = 0x4000_0000;
/// A deal not finished within this long is given up (its transaction expires anyway).
const DEAL_SECS: u64 = 2 * 3600;

fn now() -> u64 {
    krypton_core::now()
}

/// GNR without trailing zeros: 10 GNR, 0.0000256 GNR.
fn gnr_text(v: u64) -> String {
    let s = format_amount(v);
    let s = s.trim_end_matches('0');
    s.trim_end_matches('.').to_string()
}

/// The last steps of this session's coin swaps, newest last (shown in the Coins tab).
fn events() -> &'static Mutex<std::collections::VecDeque<(u64, String)>> {
    static E: OnceLock<Mutex<std::collections::VecDeque<(u64, String)>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}

fn event(m: impl Into<String>) {
    let mut e = events().lock();
    e.push_back((now(), m.into()));
    while e.len() > 40 {
        e.pop_front();
    }
}

// ---- coin information (manifests verified against their ids) ----

#[derive(Clone)]
struct Info {
    manifest: Manifest,
    summary: Value,
}

/// When the coins were fetched, the coins, and the node's raw list.
type CoinCache = (Option<Instant>, HashMap<AssetId, Info>, Value);

fn info_cache() -> &'static Mutex<CoinCache> {
    static C: OnceLock<Mutex<CoinCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new((None, HashMap::new(), Value::Null)))
}

/// The coins on chain, refreshed at most every 10 seconds. Every manifest must hash to
/// its coin's id, so the node cannot misstate what a coin is.
fn coins(a: &App) -> (HashMap<AssetId, Info>, Value) {
    {
        let c = info_cache().lock();
        if c.0.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
            return (c.1.clone(), c.2.clone());
        }
    }
    crate::verified::refresh();
    let Ok(src) = a.sources() else {
        return (HashMap::new(), Value::Null);
    };
    let Some(c) = src.first() else {
        return (HashMap::new(), Value::Null);
    };
    let list = c.assets().unwrap_or(Value::Null);
    let mut map = HashMap::new();
    for s in list["assets"].as_array().cloned().unwrap_or_default() {
        let Some(id) = s["id"].as_str().and_then(AssetId::from_hex) else {
            continue;
        };
        if let Ok((got, manifest, full)) = c.asset(&id.to_hex())
            && got == id
        {
            map.insert(
                id,
                Info {
                    manifest,
                    summary: full,
                },
            );
        }
    }
    *info_cache().lock() = (Some(Instant::now()), map.clone(), list.clone());
    (map, list)
}

fn coin_by(a: &App, s: &str) -> Result<(AssetId, Manifest)> {
    let (map, _) = coins(a);
    let s = s.trim();
    if let Some(id) = AssetId::from_hex(s)
        && let Some(i) = map.get(&id)
    {
        return Ok((id, i.manifest.clone()));
    }
    map.iter()
        .find(|(_, i)| i.manifest.ticker.eq_ignore_ascii_case(s))
        .map(|(id, i)| (*id, i.manifest.clone()))
        .context("no such coin on chain")
}

/// Coins the project verified (a signed list; see `crate::verified`).
fn is_verified(id: &AssetId) -> bool {
    crate::verified::is_verified(id)
}

fn coin_view(id: &AssetId, i: &Info) -> Value {
    let m = &i.manifest;
    let verified = is_verified(id);
    let entry = crate::verified::list(crate::verified::network()).remove(id);
    json!({
        "verified_issuer": entry.as_ref().map(|e| e.issuer.clone()),
        "verified_logo": entry.as_ref().and_then(|e| e.logo.clone()),
        "verified_note": entry.as_ref().and_then(|e| e.note.clone()),
        "id": id, "name": m.name, "ticker": m.ticker, "decimals": m.decimals,
        "supply": m.supply, "cap": m.cap.map(|c| format_units(c, m.decimals)),
        "burn": m.burn, "pause": m.pause, "backing": m.backing,
        "origin": m.origin, "reserves": m.reserves, "website": m.website, "terms": m.terms,
        "issued": format_units(i.summary["issued"].as_u64().unwrap_or(0), m.decimals),
        "outstanding": format_units(i.summary["outstanding"].as_u64().unwrap_or(0), m.decimals),
        "burned": format_units(i.summary["burned"].as_u64().unwrap_or(0), m.decimals),
        "paused": i.summary["paused"],
        "height": i.summary["height"],
        "verified": verified,
        "risks": m.risk_labels(verified),
        "logo": m.logo.map(hex::encode),
    })
}

// ---- HTTP API ----

pub async fn list(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let (map, raw) = coins(&a);
        let g = a.wallet.lock();
        let w = g.as_ref().context("unlock the wallet first")?;
        let issuer = w.issuer_key();
        let holdings: Vec<Value> = w
            .coin_balances()
            .iter()
            .map(|b| match map.get(&b.coin) {
                Some(i) => {
                    let d = i.manifest.decimals;
                    let mut v = coin_view(&b.coin, i);
                    v["spendable"] = json!(format_units(b.spendable, d));
                    v["pending"] = json!(format_units(b.pending, d));
                    v["immature"] = json!(format_units(b.immature, d));
                    v
                }
                None => json!({"id": b.coin, "ticker": "?", "name": "Unknown coin",
                    "spendable": b.spendable.to_string(), "risks": ["The node does not know this coin."]}),
            })
            .collect();
        let mine: Vec<Value> = map
            .iter()
            .filter(|(_, i)| i.manifest.issuer == issuer)
            .map(|(id, i)| coin_view(id, i))
            .collect();
        let mut all: Vec<Value> = map.iter().map(|(id, i)| coin_view(id, i)).collect();
        all.sort_by(|x, y| {
            y["verified"].as_bool().cmp(&x["verified"].as_bool()).then(
                x["ticker"].as_str().unwrap_or("").cmp(y["ticker"].as_str().unwrap_or("")),
            )
        });
        let history: Vec<Value> = w
            .history()
            .iter()
            .rev()
            .filter(|e| e.coin.is_some())
            .take(100)
            .map(|e| {
                let c = e.coin.expect("filtered");
                let (t, d) = map
                    .get(&c)
                    .map(|i| (i.manifest.ticker.clone(), i.manifest.decimals))
                    .unwrap_or(("?".into(), 0));
                json!({"txid": e.txid, "height": e.height, "ticker": t, "coin": c,
                    "amount": format!("{}{}", if e.amount < 0 { "-" } else { "+" }, format_units(e.amount.unsigned_abs() as u64, d)),
                    "fee": gnr_text(e.fee), "memo": e.memo, "to": e.to, "time": e.time,
                    "provable": e.disclosure.is_some()})
            })
            .collect();
        Ok(json!({
            "active": raw["active"], "assets_height": raw["assets_height"],
            "deploy_fee": gnr_text(krypton_core::asset::DEPLOY_FEE),
            "holdings": holdings, "mine": mine, "all": all, "history": history,
            "issuer": hex::encode(krypton_core::asset::issuer_fingerprint(&issuer)),
            "gnr": w.balance(),
        }))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

fn pick_fee(src: &Multi, speed: Option<&str>) -> u64 {
    let (economy, normal, fast, _) = fee_options(src);
    match speed.unwrap_or("normal") {
        "economy" => economy,
        "fast" => fast,
        _ => normal,
    }
}

fn recipient(a: &App, to: &str) -> Result<Address> {
    let to: String = to.split_whitespace().collect();
    ensure!(!to.is_empty(), "enter the recipient's address");
    let full = if to.contains("s1") && to.len() < 120 {
        crate::resolve_short_any(a, &to)?
    } else {
        to
    };
    Address::decode(&full)
        .map_err(|e| anyhow!("this is not a valid {} address: {e}", a.network.as_str()))
}

#[derive(Deserialize)]
pub struct SendReq {
    coin: String,
    to: String,
    amount: String,
    #[serde(default)]
    memo: String,
    #[serde(default)]
    speed: Option<String>,
    password: String,
}

/// Runs a wallet action that proves and submits a coin transaction.
fn act<T>(
    a: &App,
    password: &str,
    busy: &str,
    f: impl FnOnce(&mut Wallet, &Multi, u64) -> Result<T>,
    speed: Option<&str>,
) -> Result<T> {
    confirm_password(a, password)?;
    let src = Multi(a.sources()?);
    src.cross_check()?;
    let fee = pick_fee(&src, speed);
    let mut g = a.wallet.lock();
    let w = g.as_mut().context("wallet locked")?;
    w.sync(&src, |_, _| {})?;
    *a.busy.lock() = busy.into();
    let r = f(w, &src, fee);
    *a.busy.lock() = String::new();
    crate::snapshot(w);
    r
}

pub async fn send(axum::extract::State(app): S, h: HeaderMap, Json(r): Json<SendReq>) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let (id, m) = coin_by(&a, &r.coin)?;
        let amount = parse_units(&r.amount, m.decimals).context("invalid amount")?;
        let to = recipient(&a, &r.to)?;
        act(
            &a,
            &r.password,
            "Proving a private coin transfer...",
            |w, src, fee| w.send_coin(src, id, &to, amount, fee, &r.memo),
            r.speed.as_deref(),
        )
    })
    .await
    {
        Ok(id) => Json(json!({"txid": hex::encode(id)})).into_response(),
        Err(e) => err(e),
    }
}

/// The fields of a new coin, as the Issue wizard collects them.
#[derive(Deserialize)]
pub struct DraftReq {
    name: String,
    ticker: String,
    decimals: u8,
    supply: String,
    #[serde(default)]
    cap: String,
    #[serde(default)]
    burn: bool,
    #[serde(default)]
    pause: bool,
    backing: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    reserves: String,
    #[serde(default)]
    website: String,
    #[serde(default)]
    terms: String,
    #[serde(default)]
    password: String,
}

fn opt(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn draft(w: &Wallet, r: &DraftReq) -> Result<Manifest> {
    let supply = match r.supply.as_str() {
        "fixed" => Supply::Fixed,
        "mintable" => Supply::Mintable,
        _ => bail!("choose fixed or mintable supply"),
    };
    let backing = match r.backing.as_str() {
        "none" => Backing::None,
        "issuer-reserves" => Backing::IssuerReserves,
        "bridge" => Backing::Bridge,
        _ => bail!("choose what backs the coin"),
    };
    let cap = opt(&r.cap)
        .map(|c| parse_units(&c, r.decimals).context("invalid cap"))
        .transpose()?;
    let m = Manifest {
        name: r.name.trim().to_string(),
        ticker: r.ticker.trim().to_ascii_uppercase(),
        decimals: r.decimals,
        supply,
        cap,
        burn: r.burn,
        pause: r.pause,
        backing,
        origin: opt(&r.origin),
        reserves: opt(&r.reserves),
        website: opt(&r.website),
        terms: opt(&r.terms),
        logo: None,
        issuer: w.issuer_key(),
    };
    m.validate()
        .map_err(|e| anyhow!("{}", e.to_string().trim_start_matches("manifest: ")))?;
    Ok(m)
}

/// The wizard's preview: the exact manifest, its id and the risks holders will see.
pub async fn preview(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<DraftReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let g = a.wallet.lock();
        let w = g.as_ref().context("wallet locked")?;
        let m = draft(w, &r)?;
        let (map, _) = coins(&a);
        let taken = map.values().any(|i| {
            i.manifest.ticker == m.ticker || i.manifest.name.eq_ignore_ascii_case(&m.name)
        });
        Ok(json!({
            "id": m.id(), "risks": m.risk_labels(false), "taken": taken,
            "manifest": m.canonical().lines().filter(|l| !l.starts_with("issuer:")).collect::<Vec<_>>().join("\n"),
            "deploy_fee": gnr_text(krypton_core::asset::DEPLOY_FEE),
            "major": krypton_assets::MAJOR_TICKERS.contains(&m.ticker.as_str()),
        }))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

pub async fn deploy(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<DraftReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let m = {
            let g = a.wallet.lock();
            draft(g.as_ref().context("wallet locked")?, &r)?
        };
        let id = m.id();
        let txid = act(
            &a,
            &r.password,
            "Deploying the coin...",
            |w, src, fee| w.deploy_coin(src, &m, fee),
            None,
        )?;
        *info_cache().lock() = (None, HashMap::new(), Value::Null);
        Ok(json!({"txid": hex::encode(txid), "id": id}))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct IssuerReq {
    coin: String,
    /// issue, burn, pause or unpause
    action: String,
    #[serde(default)]
    amount: String,
    /// Issue: the recipient (empty: this wallet).
    #[serde(default)]
    to: String,
    password: String,
}

pub async fn issuer(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<IssuerReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let (id, m) = coin_by(&a, &r.coin)?;
        let units = || parse_units(&r.amount, m.decimals).context("invalid amount");
        let txid = match r.action.as_str() {
            "issue" => {
                let amount = units()?;
                let to = if r.to.trim().is_empty() {
                    let g = a.wallet.lock();
                    g.as_ref().context("wallet locked")?.address()
                } else {
                    recipient(&a, &r.to)?
                };
                act(
                    &a,
                    &r.password,
                    "Issuing...",
                    |w, src, fee| w.issue_coin(src, id, &to, amount, fee),
                    None,
                )?
            }
            "burn" => {
                let amount = units()?;
                act(
                    &a,
                    &r.password,
                    "Burning...",
                    |w, src, fee| w.burn_coin(src, id, amount, fee),
                    None,
                )?
            }
            "pause" | "unpause" => {
                let p = r.action == "pause";
                act(
                    &a,
                    &r.password,
                    "Updating the coin...",
                    |w, src, fee| w.set_coin_paused(src, id, p, fee),
                    None,
                )?
            }
            _ => bail!("unknown action"),
        };
        *info_cache().lock() = (None, HashMap::new(), Value::Null);
        Ok(hex::encode(txid))
    })
    .await
    {
        Ok(id) => Json(json!({"txid": id})).into_response(),
        Err(e) => err(e),
    }
}

// ---- the coin market ----

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Role {
    /// Pays GNR, receives the coin.
    Buyer,
    /// Pays the coin, receives GNR.
    Seller,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MyOffer {
    n: u32,
    offer: CoinOffer,
    cancel_secret: String,
    taken: bool,
    cancelled: bool,
}

/// One swap with one counterparty.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Deal {
    offer: String,
    n: u32,
    peer: String,
    role: Role,
    maker: bool,
    terms: SwapTerms,
    started: u64,
    /// `asked`, `half_sent`, `tx_sent`, `submitted`, `done` or `failed: ...`.
    stage: String,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    receipt: Option<Value>,
    #[serde(default)]
    reported: bool,
    /// Seller: the history id of our side of the swap (confirms when mined).
    #[serde(default)]
    watch: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Book {
    next: u32,
    offers: Vec<MyOffer>,
    deals: Vec<Deal>,
    seen: Vec<String>,
}

fn load(w: &Wallet) -> Book {
    w.app_data(STORE_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn save(w: &mut Wallet, b: &Book) -> Result<()> {
    w.set_app_data(STORE_KEY, serde_json::to_value(b)?)
}

fn key(w: &Wallet, n: u32) -> krypton_crypto::kem::SecretKey {
    offer_key(&w.keys.swap_seed, KEY_BASE + n)
}

fn send_note(
    base: &str,
    to_hex: &str,
    from: &krypton_crypto::kem::SecretKey,
    note: &Note,
) -> Result<()> {
    let to = krypton_crypto::kem::PublicKey::from_bytes(&hex::decode(to_hex)?)?;
    let env = board::seal(&to, from, &serde_json::to_vec(note)?)?;
    crate::board::post_json(base, "/v1/swap/inbox", &env)?;
    Ok(())
}

pub async fn market(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let base = crate::board::board_url(&a);
        let raw = crate::board::get_json(&base, "/v1/swap/coins").unwrap_or_default();
        let (map, _) = coins(&a);
        let book = a.wallet.lock().as_ref().map(load).unwrap_or_default();
        let mine: Vec<String> = book.offers.iter().map(|o| o.offer.id.clone()).collect();
        let t = now();
        let view = |o: &Value| -> Option<Value> {
            let id = o["coin"].as_str().and_then(AssetId::from_hex)?;
            let i = map.get(&id)?;
            let d = i.manifest.decimals;
            let (g, u) = (o["gnr"].as_u64()?, o["units"].as_u64()?);
            let whole = u as f64 / 10f64.powi(d as i32);
            Some(json!({
                "id": o["id"], "coin": id, "ticker": i.manifest.ticker, "name": i.manifest.name,
                "verified": is_verified(&id), "side": o["side"],
                "gnr": gnr_text(g), "units": format_units(u, d),
                "price": if whole > 0.0 { g as f64 / 1e8 / whole } else { 0.0 },
                "expires_in": o["expires"].as_u64().unwrap_or(0).saturating_sub(t),
                "online": o["online"].as_bool().unwrap_or(false) || mine.contains(&o["id"].as_str().unwrap_or("").to_string()),
                "mine": mine.iter().any(|m| Some(m.as_str()) == o["id"].as_str()),
            }))
        };
        let offers: Vec<Value> = raw["offers"].as_array().into_iter().flatten().filter_map(view).collect();
        let markets: Value = raw["markets"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        let id = AssetId::from_hex(k)?;
                        let i = map.get(&id)?;
                        let per_unit = v["price"].as_f64()?;
                        Some((i.manifest.ticker.clone(), json!({
                            "coin": id,
                            "price": per_unit * 10f64.powi(i.manifest.decimals as i32) / 1e8,
                            "swaps": v["swaps"],
                        })))
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into()
            })
            .unwrap_or(Value::Null);
        let deals: Vec<Value> = book
            .deals
            .iter()
            .rev()
            .take(50)
            .map(|d| {
                let (tk, dec) = map
                    .get(&d.terms.coin)
                    .map(|i| (i.manifest.ticker.clone(), i.manifest.decimals))
                    .unwrap_or(("?".into(), 0));
                json!({"offer": d.offer, "role": d.role, "maker": d.maker, "stage": d.stage,
                    "ticker": tk, "gnr": gnr_text(d.terms.gnr), "units": format_units(d.terms.units, dec),
                    "txid": d.txid, "started": d.started})
            })
            .collect();
        let log: Vec<Value> = events().lock().iter().rev().take(15).map(|(t, m)| json!({"at": t, "event": m})).collect();
        Ok(json!({"board": base, "offers": offers, "markets": markets, "deals": deals, "events": log}))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct OfferReq {
    coin: String,
    /// `sell_gnr` (buy the coin with GNR) or `buy_gnr` (sell the coin for GNR).
    side: Side,
    gnr: String,
    units: String,
    #[serde(default)]
    hours: Option<u64>,
    password: String,
}

pub async fn post_offer(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<OfferReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        confirm_password(&a, &r.password)?;
        let (id, m) = coin_by(&a, &r.coin)?;
        let gnr = parse_amount(&r.gnr).context("invalid GNR amount")?;
        let units = parse_units(&r.units, m.decimals).context("invalid coin amount")?;
        ensure!(gnr > 0 && units > 0, "both amounts must be positive");
        let base = crate::board::board_url(&a);
        let mut g = a.wallet.lock();
        let w = g.as_mut().context("wallet locked")?;
        match r.side {
            Side::SellGnr => ensure!(
                w.balance().spendable > gnr,
                "not enough spendable GNR for this offer"
            ),
            Side::BuyGnr => ensure!(
                w.coin_balance(id) >= units,
                "not enough {} for this offer",
                m.ticker
            ),
        }
        let mut book = load(w);
        let n = book.next;
        book.next += 1;
        let k = key(w, n);
        let secret: [u8; 32] = rand::random();
        let secs = r.hours.unwrap_or(24).clamp(1, board::MAX_OFFER_SECS / 3600) * 3600;
        let mut offer = CoinOffer {
            id: hex::encode(rand::random::<[u8; 16]>()),
            network: a.network.as_str().into(),
            coin: id.to_hex(),
            side: r.side,
            gnr,
            units,
            key: hex::encode(k.public().to_bytes()),
            expires: now() + secs,
            cancel: board::cancel_hash(&secret),
            nonce: 0,
        };
        *a.busy.lock() = "Posting the offer (a little proof of work)...".into();
        offer.seal_pow();
        *a.busy.lock() = String::new();
        crate::board::post_json(&base, "/v1/swap/coins", &offer)?;
        book.offers.push(MyOffer {
            n,
            offer: offer.clone(),
            cancel_secret: hex::encode(secret),
            taken: false,
            cancelled: false,
        });
        save(w, &book)?;
        Ok(offer.id)
    })
    .await
    {
        Ok(id) => Json(json!({"id": id})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct IdReq {
    id: String,
    #[serde(default)]
    password: String,
}

pub async fn cancel_offer(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<IdReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let base = crate::board::board_url(&a);
        let mut g = a.wallet.lock();
        let w = g.as_mut().context("wallet locked")?;
        let mut book = load(w);
        let m = book
            .offers
            .iter_mut()
            .find(|o| o.offer.id == r.id)
            .context("not your offer")?;
        crate::board::post_json(
            &base,
            "/v1/swap/offers/cancel",
            &json!({"id": m.offer.id, "secret": m.cancel_secret}),
        )?;
        m.cancelled = true;
        save(w, &book)
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn take(axum::extract::State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        confirm_password(&a, &r.password)?;
        let base = crate::board::board_url(&a);
        let raw = crate::board::get_json(&base, "/v1/swap/coins")?;
        let o: CoinOffer = raw["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["id"].as_str() == Some(r.id.as_str()))
            .and_then(|o| serde_json::from_value(o.clone()).ok())
            .context("the offer is no longer on the board")?;
        o.validate(a.network.as_str(), now())?;
        let coin = o.coin_id()?;
        let (_, m) = coin_by(&a, &coin.to_hex())?;
        let mut g = a.wallet.lock();
        let w = g.as_mut().context("wallet locked")?;
        // The maker sells GNR: we sell the coin. The maker buys GNR: we buy the coin.
        let role = if o.side == Side::SellGnr {
            Role::Seller
        } else {
            Role::Buyer
        };
        match role {
            Role::Seller => ensure!(
                w.coin_balance(coin) >= o.units,
                "you do not hold enough {}",
                m.ticker
            ),
            Role::Buyer => ensure!(w.balance().spendable > o.gnr, "not enough spendable GNR"),
        }
        let mut book = load(w);
        ensure!(
            !book.offers.iter().any(|x| x.offer.id == o.id),
            "this is your own offer"
        );
        let n = book.next;
        book.next += 1;
        send_note(
            &base,
            &o.key,
            &key(w, n),
            &Note::CoinTake {
                offer: o.id.clone(),
                address: w.address().encode(),
            },
        )?;
        book.deals.push(Deal {
            offer: o.id.clone(),
            n,
            peer: o.key.clone(),
            role,
            maker: false,
            terms: SwapTerms {
                coin,
                units: o.units,
                gnr: o.gnr,
            },
            started: now(),
            stage: "asked".into(),
            txid: None,
            receipt: None,
            reported: false,
            watch: None,
        });
        save(w, &book)?;
        Ok(o.id)
    })
    .await
    {
        Ok(id) => Json(json!({"id": id})).into_response(),
        Err(e) => err(e),
    }
}

// ---- the background round: mailboxes and reports ----

pub fn has_work(w: &Wallet) -> bool {
    let b = load(w);
    if b.deals.iter().any(|d| d.stage == "tx_sent") {
        return true;
    }
    let t = now();
    b.offers
        .iter()
        .any(|o| !o.cancelled && !o.taken && o.offer.expires > t)
        || b.deals
            .iter()
            .any(|d| !finished(d) || (d.receipt.is_some() && !d.reported))
}

/// Runs `f` on a freshly synced wallet, once more if a block arrived in between (a spend
/// must prove against the tip the wallet verified).
fn fresh<T>(w: &mut Wallet, src: &Multi, mut f: impl FnMut(&mut Wallet) -> Result<T>) -> Result<T> {
    let mut last = None;
    for _ in 0..3 {
        let _ = w.sync(src, |_, _| {});
        match f(w) {
            Ok(v) => return Ok(v),
            Err(e)
                if e.to_string().contains("sync") || e.to_string().contains("chain advanced") =>
            {
                last = Some(e)
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("the chain keeps moving; try again")))
}

fn finished(d: &Deal) -> bool {
    d.stage == "done" || d.stage.starts_with("failed") || d.stage == "submitted"
}

fn fail(d: &mut Deal, why: impl std::fmt::Display) {
    d.stage = format!("failed: {why}");
}

pub fn tick(a: &App, w: &mut Wallet, src: &Multi) {
    if !has_work(w) {
        return;
    }
    if let Err(x) = round(a, w, src) {
        *a.last_error.lock() = format!("coin market: {x}");
    }
}

/// Tells the board our open coin offers are online (at most once a minute): only offers
/// of makers seen lately can be taken.
fn ping(base: &str, book: &Book) {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(None));
    if last
        .lock()
        .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
    {
        return;
    }
    let t = now();
    let items: Vec<Value> = book
        .offers
        .iter()
        .filter(|o| !o.cancelled && !o.taken && o.offer.expires > t)
        .map(|o| json!({"id": o.offer.id, "secret": o.cancel_secret}))
        .collect();
    if items.is_empty() {
        return;
    }
    *last.lock() = Some(Instant::now());
    if let Err(e) =
        crate::board::post_json(base, "/v1/swap/offers/ping", &json!({ "items": items }))
    {
        event(format!("ping: {e}"));
    }
}

fn round(a: &App, w: &mut Wallet, src: &Multi) -> Result<()> {
    let base = crate::board::board_url(a);
    let mut book = load(w);
    let t = now();
    ping(&base, &book);
    let mut keys: Vec<u32> = book
        .offers
        .iter()
        .filter(|o| !o.cancelled && o.offer.expires > t)
        .map(|o| o.n)
        .collect();
    keys.extend(book.deals.iter().filter(|d| !finished(d)).map(|d| d.n));
    keys.sort_unstable();
    keys.dedup();
    let fee = pick_fee(src, None);
    for n in keys {
        let k = key(w, n);
        let inbox = board::inbox_of(k.public());
        let v = match crate::board::get_json(&base, &format!("/v1/swap/inbox/{inbox}")) {
            Ok(v) => v,
            Err(e) => {
                event(format!("mailbox {n}: {e}"));
                continue;
            }
        };
        for m in v["messages"].as_array().cloned().unwrap_or_default() {
            let Some(data) = m["data"].as_str() else {
                continue;
            };
            let h = hex::encode(krypton_crypto::hash::tagged(
                "Krypton/coin-board/seen",
                data.as_bytes(),
            ));
            if book.seen.contains(&h) {
                continue;
            }
            book.seen.push(h);
            let Ok((from, body)) = board::open(&k, data) else {
                event(format!("mailbox {n}: a message not for us"));
                continue;
            };
            let note = match serde_json::from_slice::<Note>(&body) {
                Ok(x) => x,
                Err(e) => {
                    event(format!("mailbox {n}: unreadable message ({e})"));
                    continue;
                }
            };
            let from = hex::encode(from.to_bytes());
            let kind = match &note {
                Note::CoinTake { .. } => "take",
                Note::CoinHalf { .. } => "GNR half",
                Note::CoinTx { .. } => "transaction",
                Note::Gone { .. } => "gone",
                _ => "other",
            };
            match handle(w, src, &base, &mut book, n, &from, note, fee) {
                Ok(()) => event(format!("received the {kind}")),
                Err(x) => {
                    event(format!("the {kind} failed: {x}"));
                    *a.last_error.lock() = format!("coin swap: {x}");
                }
            }
        }
    }
    if book.seen.len() > 2000 {
        book.seen.drain(..1000);
    }
    // Sellers learn the outcome from the chain: our change appears, or the swap expired.
    for d in book.deals.iter_mut().filter(|d| d.stage == "tx_sent") {
        let Some(id) = &d.watch else { continue };
        match w.history().iter().find(|h| &h.txid == id) {
            Some(h) if h.height.is_some() => d.stage = "done".into(),
            Some(_) => {}
            None => fail(d, "the buyer did not finish in time; nothing moved"),
        }
    }
    // Give up on deals that stalled; their transaction (if any) expires on chain.
    for d in book.deals.iter_mut().filter(|d| !finished(d)) {
        if d.started + DEAL_SECS < t {
            fail(d, "timed out");
        }
    }
    // Report our completed buys once mined, so they count for the coin's price.
    for d in book
        .deals
        .iter_mut()
        .filter(|d| d.receipt.is_some() && !d.reported)
    {
        let r = d.receipt.clone().expect("receipt");
        let trade = trade_of(a, &r)?;
        match crate::board::post_json(&base, "/v1/swap/coin-trade", &trade) {
            Ok(_) => {
                d.reported = true;
                d.stage = "done".into();
            }
            Err(e) if e.to_string().contains("not mined") => {}
            Err(_) if d.started + DEAL_SECS < t => d.reported = true,
            Err(_) => {}
        }
    }
    save(w, &book)
}

fn trade_of(a: &App, r: &Value) -> Result<CoinTrade> {
    let rc: ReceiptJson = serde_json::from_value(r.clone())?;
    let mut t = CoinTrade {
        network: a.network.as_str().into(),
        coin: rc.terms.coin.to_hex(),
        txid: hex::encode(rc.txid),
        gnr: rc.terms.gnr,
        units: rc.terms.units,
        coin_output: Opening {
            output: rc.coin_output.0,
            key: hex::encode(rc.coin_output.1),
            owner: hex::encode(rc.coin_output.2),
        },
        gnr_output: Opening {
            output: rc.gnr_output.0,
            key: hex::encode(rc.gnr_output.1),
            owner: hex::encode(rc.gnr_output.2),
        },
        nonce: 0,
    };
    t.seal_pow();
    Ok(t)
}

#[derive(Deserialize)]
struct ReceiptJson {
    #[serde(with = "hex::serde")]
    txid: [u8; 32],
    terms: SwapTerms,
    coin_output: (u8, [u8; 32], [u8; 32]),
    gnr_output: (u8, [u8; 32], [u8; 32]),
}

fn receipt_json(r: &SwapReceipt) -> Value {
    json!({"txid": hex::encode(r.txid), "terms": r.terms, "coin_output": r.coin_output, "gnr_output": r.gnr_output})
}

#[allow(clippy::too_many_arguments)]
fn handle(
    w: &mut Wallet,
    src: &Multi,
    base: &str,
    book: &mut Book,
    n: u32,
    from: &str,
    note: Note,
    fee: u64,
) -> Result<()> {
    let k = key(w, n);
    match note {
        Note::CoinTake { offer, address } => {
            if let Some(mo) = book
                .offers
                .iter_mut()
                .find(|o| o.n == n && o.offer.id == offer)
            {
                // Someone takes our offer: the first taker gets it.
                if mo.taken || mo.cancelled || mo.offer.expires <= now() {
                    return send_note(base, from, &k, &Note::Gone { offer });
                }
                let coin = mo.offer.coin_id()?;
                let terms = SwapTerms {
                    coin,
                    units: mo.offer.units,
                    gnr: mo.offer.gnr,
                };
                let role = if mo.offer.side == Side::SellGnr {
                    Role::Buyer
                } else {
                    Role::Seller
                };
                mo.taken = true;
                let _ = crate::board::post_json(
                    base,
                    "/v1/swap/offers/cancel",
                    &json!({"id": mo.offer.id, "secret": mo.cancel_secret}),
                );
                let mut d = Deal {
                    offer: offer.clone(),
                    n,
                    peer: from.into(),
                    role: role.clone(),
                    maker: true,
                    terms,
                    started: now(),
                    stage: "asked".into(),
                    txid: None,
                    receipt: None,
                    reported: false,
                    watch: None,
                };
                match role {
                    Role::Buyer => {
                        let seller = Address::decode(&address)?;
                        match fresh(w, src, |w| w.coin_swap_half(src, terms, &seller, fee)) {
                            Ok(half) => match send_note(
                                base,
                                from,
                                &k,
                                &Note::CoinHalf {
                                    offer,
                                    half: serde_json::to_string(&half)?,
                                },
                            ) {
                                Ok(()) => d.stage = "half_sent".into(),
                                Err(e) => {
                                    w.coin_swap_drop(&half);
                                    fail(&mut d, format!("could not reach the other wallet: {e}"));
                                }
                            },
                            Err(e) => fail(&mut d, e),
                        }
                    }
                    Role::Seller => {
                        // The buyer needs our address to pay us.
                        send_note(
                            base,
                            from,
                            &k,
                            &Note::CoinTake {
                                offer,
                                address: w.address().encode(),
                            },
                        )?;
                    }
                }
                book.deals.push(d);
                return Ok(());
            }
            // We took an offer as the buyer: the seller answered with its address.
            let Some(d) = book
                .deals
                .iter_mut()
                .find(|d| d.n == n && d.offer == offer && d.peer == from)
            else {
                return Ok(());
            };
            if d.role != Role::Buyer || d.stage != "asked" {
                return Ok(());
            }
            let seller = Address::decode(&address)?;
            let terms = d.terms;
            match fresh(w, src, |w| w.coin_swap_half(src, terms, &seller, fee)) {
                Ok(half) => match send_note(
                    base,
                    from,
                    &k,
                    &Note::CoinHalf {
                        offer,
                        half: serde_json::to_string(&half)?,
                    },
                ) {
                    Ok(()) => d.stage = "half_sent".into(),
                    Err(e) => {
                        w.coin_swap_drop(&half);
                        fail(d, format!("could not reach the other wallet: {e}"));
                    }
                },
                Err(e) => fail(d, e),
            }
            Ok(())
        }
        Note::CoinHalf { offer, half } => {
            let Some(d) = book
                .deals
                .iter_mut()
                .find(|d| d.n == n && d.offer == offer && d.peer == from)
            else {
                return Ok(());
            };
            if d.role != Role::Seller || d.stage != "asked" {
                return Ok(());
            }
            let half: GnrHalf = serde_json::from_str(&half)?;
            let terms = d.terms;
            match fresh(w, src, |w| w.coin_swap_accept(src, &half, terms)) {
                Ok(tx) => {
                    let note = Note::CoinTx {
                        offer,
                        tx: krypton_core::address::base32_encode(&tx.to_bytes()),
                    };
                    d.watch = Some(format!("swap:{}", hex::encode(tx.outputs[1].cm)));
                    match send_note(base, from, &k, &note) {
                        Ok(()) => d.stage = "tx_sent".into(),
                        // Our coins stay reserved until the transaction expires (minutes).
                        Err(e) => fail(d, format!("could not reach the other wallet: {e}")),
                    }
                }
                Err(e) => {
                    let _ = send_note(base, from, &k, &Note::Gone { offer });
                    fail(d, e);
                }
            }
            Ok(())
        }
        Note::CoinTx { offer, tx } => {
            let Some(d) = book
                .deals
                .iter_mut()
                .find(|d| d.n == n && d.offer == offer && d.peer == from)
            else {
                return Ok(());
            };
            if d.role != Role::Buyer || d.stage != "half_sent" {
                return Ok(());
            }
            let bytes =
                krypton_core::address::base32_decode(&tx).context("bad transaction encoding")?;
            let tx = krypton_core::Transaction::from_bytes(&bytes)?;
            let _ = w.sync(src, |_, _| {});
            match w.coin_swap_finish(src, tx) {
                Ok(r) => {
                    d.txid = Some(hex::encode(r.txid));
                    d.receipt = Some(receipt_json(&r));
                    d.stage = "submitted".into();
                }
                Err(e) => fail(d, e),
            }
            Ok(())
        }
        Note::Gone { offer } => {
            for d in book
                .deals
                .iter_mut()
                .filter(|d| d.n == n && d.offer == offer && d.peer == from && !finished(d))
            {
                fail(d, "the other side ended the swap");
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
