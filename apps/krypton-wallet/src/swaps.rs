//! Atomic swaps in the desktop wallet: the HTTP API for the Swap tab and the background
//! step that advances every swap of the active wallet (see `krypton_swap::engine`).

use crate::{App, Multi, S, blocking, check_open, err};
use anyhow::{Context, Result, anyhow};
use axum::Json;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use krypton_core::Network;
use krypton_core::emission::{format_amount, parse_amount};
use krypton_swap::bitcoin::{Amount, Denomination};
use krypton_swap::engine::{self, Engine, Stage, Swap};
use krypton_swap::{Timing, btc};
use krypton_wallet_core::Wallet;
use serde::Deserialize;
use serde_json::{Value, json};

/// The Bitcoin network paired with a GNR network.
pub fn btc_network(a: &App) -> krypton_swap::bitcoin::Network {
    use krypton_swap::bitcoin::Network as B;
    // Real GNR only ever trades against real bitcoin.
    if a.network == Network::Mainnet {
        return B::Bitcoin;
    }
    let set = a.settings.lock().btc_network.clone();
    if let Some(n) = set.as_deref()
        && let Ok(n) = B::from_core_arg(n)
    {
        return n;
    }
    match a.network {
        Network::Mainnet => B::Bitcoin,
        Network::Testnet => B::Signet,
        Network::Devnet => B::Regtest,
    }
}

pub(crate) fn backend_spec(a: &App) -> String {
    // Release the settings lock first: btc_network takes it again.
    let set = a.settings.lock().btc_backend.clone();
    set.filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| match btc_network(a) {
            krypton_swap::bitcoin::Network::Regtest => "rpc:http://gk:gk@127.0.0.1:18443".into(),
            n => format!("esplora:{}", btc::default_esplora(n)),
        })
}

/// Why swaps cannot be used on this network yet, if they cannot.
pub(crate) fn inactive(a: &App, w: Option<&Wallet>) -> Option<String> {
    let at = a.network.params().lock_height;
    let tip = w.and_then(|w| w.synced_height()).unwrap_or(0);
    (tip.saturating_add(1) < at).then(|| {
        if at == u64::MAX {
            format!(
                "Atomic swaps need a GNR network upgrade that has not been scheduled on {} yet. They already run on devnet for testing.",
                a.network.as_str()
            )
        } else {
            format!("Atomic swaps activate at block {at}.")
        }
    })
}

fn view(s: &Swap, via_board: bool) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    if via_board {
        // The board carries the codes: there is nothing to copy or send by hand.
        v["code"] = Value::Null;
        v["via_board"] = json!(true);
        let st = match (s.stage, s.gnr_first()) {
            (Stage::Requested, _) => {
                Some("Request sent through the board; waiting for the seller's answer.")
            }
            (Stage::Accepted, false) => {
                Some("Accepted through the board; waiting for the buyer's bitcoin deposit.")
            }
            (Stage::Deposited, false) if s.role == engine::Role::BtcSeller => {
                Some("Deposit seen; the seller is told through the board and locks the GNR next.")
            }
            _ => None,
        };
        if let Some(t) = st {
            v["status"] = json!(t);
        }
    }
    v["gnr_text"] = json!(format_amount(s.gnr));
    v["btc_text"] = json!(Amount::from_sat(s.sats).to_string_in(Denomination::Bitcoin));
    v["receive_text"] = json!(match s.role {
        engine::Role::BtcSeller =>
            format!("{} GNR", format_amount(s.gnr.saturating_sub(s.claim_fee))),
        engine::Role::GnrSeller => format!(
            "{} BTC",
            Amount::from_sat(s.sats).to_string_in(Denomination::Bitcoin)
        ),
    });
    // GNR first: the buyer is shown where to pay only once the GNR is locked for it.
    let pay_now = !s.gnr_first()
        || s.role != engine::Role::BtcSeller
        || !matches!(s.stage, Stage::Requested | Stage::Accepted);
    v["htlc_address"] = json!(
        s.htlc_address()
            .ok()
            .filter(|_| pay_now)
            .map(|a| a.to_string())
    );
    v["gnr_first"] = json!(s.gnr_first());
    v["pay_in"] = json!(s.pay_by.map(|p| p.saturating_sub(krypton_core::now())));
    v["finished"] = json!(s.stage.finished());
    v["cancellable"] = json!(s.cancellable());
    if s.cancellable() {
        let grace = if s.role == engine::Role::GnrSeller {
            3600
        } else {
            0
        };
        let end = s.created + engine::DEPOSIT_TIMEOUT_SECS + grace;
        v["expires_in"] = json!(end.saturating_sub(krypton_core::now()));
    }
    v
}

fn run<T>(a: &App, f: impl FnOnce(&mut Engine<'_, Multi>) -> Result<T>) -> Result<T> {
    let src = Multi(a.sources()?);
    let backend = btc::connect(&backend_spec(a))?;
    let net = btc_network(a);
    let mut guard = a.wallet.lock();
    let w = guard.as_mut().context("wallet locked")?;
    if let Some(why) = inactive(a, Some(w)) {
        anyhow::bail!(why);
    }
    let blocks = |h: u64| -> Result<Vec<krypton_core::types::Transaction>> {
        Ok(src.first()?.raw_block(h)?.txs)
    };
    let mut e = Engine {
        wallet: w,
        gnr: &src,
        btc: backend.as_ref(),
        btc_network: net,
        timing: Timing::for_network(net),
        block_txs: &blocks,
    };
    f(&mut e)
}

pub async fn list(axum::extract::State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let g = app.wallet.lock();
    let swaps = g.as_ref().map(engine::load).unwrap_or_default();
    let linked = g.as_ref().map(crate::board::linked_ids).unwrap_or_default();
    let why = inactive(&app, g.as_ref());
    drop(g);
    let mut swaps: Vec<Value> = swaps
        .iter()
        .map(|s| view(s, linked.contains(&s.id)))
        .collect();
    swaps.reverse();
    Json(json!({
        "active": why.is_none(),
        "reason": why,
        "btc_network": btc_network(&app).to_core_arg(),
        "btc_backend": backend_spec(&app),
        "timing": format!("{:?}", Timing::for_network(btc_network(&app))),
        "swaps": swaps,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct RequestReq {
    gnr: String,
    btc: String,
    address: String,
    #[serde(default)]
    password: String,
}

pub async fn request(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<RequestReq>,
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
        run(&a, |e| e.request(gnr, sats, &r.address))
    })
    .await
    {
        Ok(s) => Json(view(&s, false)).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct CodeReq {
    code: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    password: String,
}

#[derive(Deserialize)]
pub struct IdReq {
    id: String,
}

/// Cancels a swap before anything is locked; the board tells the other side.
pub async fn cancel(axum::extract::State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        run(&a, |e| {
            e.cancel(&r.id, "Cancelled by you. Nothing was locked.")
        })
    })
    .await
    {
        Ok(s) => Json(view(&s, false)).into_response(),
        Err(e) => err(e),
    }
}

/// What a code says, before acting on it (the Accept dialog shows the amounts).
pub async fn decode(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<CodeReq>,
) -> Response {
    if let Err(x) = crate::check(&app, &h) {
        return x;
    }
    match engine::Msg::decode(&r.code) {
        Ok(m) => {
            let mut v = serde_json::to_value(&m).unwrap_or_default();
            if let engine::Msg::Request2 {
                gnr,
                sats,
                claim_fee,
                ..
            }
            | engine::Msg::Request {
                gnr,
                sats,
                claim_fee,
                ..
            } = &m
            {
                v["gnr_text"] = json!(format_amount(*gnr));
                v["btc_text"] = json!(Amount::from_sat(*sats).to_string_in(Denomination::Bitcoin));
                v["fee_text"] = json!(format_amount(*claim_fee));
            }
            Json(v).into_response()
        }
        Err(e) => err(e),
    }
}

pub async fn accept(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<CodeReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        crate::confirm_password(&a, &r.password)?;
        let addr = r.address.clone().unwrap_or_default();
        run(&a, |e| e.accept(&r.code, &addr))
    })
    .await
    {
        Ok(s) => Json(view(&s, false)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn receive(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<CodeReq>,
) -> Response {
    if let Err(x) = check_open(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let s = run(&a, |e| e.receive(&r.code))?;
        // Act on it right away instead of waiting for the next background round.
        let _ = run(&a, |e| e.step(&[]));
        Ok(s)
    })
    .await
    {
        Ok(s) => Json(view(&s, false)).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct SettingsReq {
    backend: String,
    #[serde(default)]
    network: Option<String>,
}

/// The Bitcoin connection: an Esplora API over https (or on this computer), or a Bitcoin
/// Core node on this computer. Empty = the default for the network.
pub async fn settings(
    axum::extract::State(app): S,
    h: HeaderMap,
    Json(r): Json<SettingsReq>,
) -> Response {
    if let Err(x) = crate::check(&app, &h) {
        return x;
    }
    let b = r.backend.trim().to_string();
    let local = |u: &str| u.starts_with("http://127.0.0.1") || u.starts_with("http://localhost");
    let ok = b.is_empty()
        || b.strip_prefix("esplora:")
            .is_some_and(|u| u.starts_with("https://") || local(u))
        || b.strip_prefix("rpc:").is_some_and(local);
    if !ok || b.len() > 300 || b.chars().any(char::is_whitespace) {
        return err(
            "use esplora:https://… or a node on this computer (rpc:http://user:pass@127.0.0.1:8332)",
        );
    }
    let net = r
        .network
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if let Some(n) = &net
        && krypton_swap::bitcoin::Network::from_core_arg(n).is_err()
    {
        return err("Bitcoin network: main, signet, testnet4 or regtest");
    }
    {
        let mut s = app.settings.lock();
        s.btc_backend = (!b.is_empty()).then_some(b);
        s.btc_network = net;
    }
    match app.save_settings() {
        Ok(()) => Json(json!({"ok": true, "btc_backend": backend_spec(&app), "btc_network": btc_network(&app).to_core_arg()})).into_response(),
        Err(e) => err(e),
    }
}

/// Background round, with the active wallet already synced and locked by the caller.
/// Whether a wallet has swaps in progress or offers on the board to watch.
pub fn has_work(w: &Wallet) -> bool {
    !engine::load(w).iter().all(|s| s.stage.finished()) || crate::board::has_work(w)
}

pub fn tick(a: &App, w: &mut Wallet, src: &Multi, mempool: &[krypton_core::types::Transaction]) {
    crate::board::refresh_market(a);
    if !has_work(w) || inactive(a, Some(w)).is_some() {
        return;
    }
    let Ok(backend) = btc::connect(&backend_spec(a)) else {
        return;
    };
    let net = btc_network(a);
    let blocks = |h: u64| -> Result<Vec<krypton_core::types::Transaction>> {
        Ok(src.first()?.raw_block(h)?.txs)
    };
    let mut e = Engine {
        wallet: w,
        gnr: src,
        btc: backend.as_ref(),
        btc_network: net,
        timing: Timing::for_network(net),
        block_txs: &blocks,
    };
    if let Err(x) = e.step(mempool) {
        *a.last_error.lock() = format!("swaps: {x}");
    }
    crate::board::tick(a, &mut e);
}

/// Between regular rounds: only the board mailboxes, so a take is answered within seconds.
pub fn quick(a: &App, w: &mut Wallet, src: &Multi) {
    if !crate::board::quick_work(w) || inactive(a, Some(w)).is_some() {
        return;
    }
    let Ok(backend) = btc::connect(&backend_spec(a)) else {
        return;
    };
    let net = btc_network(a);
    let blocks = |h: u64| -> Result<Vec<krypton_core::types::Transaction>> {
        Ok(src.first()?.raw_block(h)?.txs)
    };
    let mut e = Engine {
        wallet: w,
        gnr: src,
        btc: backend.as_ref(),
        btc_network: net,
        timing: Timing::for_network(net),
        block_txs: &blocks,
    };
    crate::board::tick(a, &mut e);
}

pub fn active_count(w: &Wallet) -> usize {
    engine::load(w)
        .iter()
        .filter(|s| !s.stage.finished() && s.stage != Stage::Failed)
        .count()
}
