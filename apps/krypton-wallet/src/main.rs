// A desktop app: no console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! krypton-wallet: desktop wallet with a local web interface.
//!
//! The UI is served on 127.0.0.1 only and every API call needs a random session
//! token that is passed to the browser in the URL fragment (never sent to a server).

use anyhow::{Context, Result, anyhow};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use krypton_core::Network;
use krypton_core::address::Address;
use krypton_core::emission::parse_amount;
use krypton_crypto::kdf::Argon2Params;
use krypton_light::client::NodeClient;
use krypton_wallet_core::{ChainSource, DEFAULT_FEE, Wallet, keys};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

mod assets;
mod update;
mod vault;

/// The project's public mainnet node (genoryn.io explorer and light wallets).
const PUBLIC_NODE: &str = "https://genoryn.io";

const UI: &str = include_str!("ui.html");

async fn asset(axum::extract::Path(p): axum::extract::Path<String>) -> Response {
    match assets::get(&p) {
        Some((b, ct)) => (
            [
                (header::CONTENT_TYPE, ct),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            b,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Parser)]
#[command(
    name = "krypton-wallet",
    version,
    about = "Genoryn Krypton desktop wallet"
)]
struct Args {
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// Port for the local interface (0 = random)
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Do not open a browser window
    #[arg(long)]
    no_browser: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    /// "light" (remote nodes, verified locally) or "full" (own node)
    mode: String,
    nodes: Vec<String>,
}

#[derive(Default, Clone, Serialize)]
struct MinerStats {
    running: bool,
    backend: String,
    hashrate: f64,
    blocks: u64,
    shares: u64,
    device: String,
    last: String,
    /// This miner's fraction of the network hashrate and the GNR/day it implies.
    network_fraction: f64,
    expected_per_day: f64,
    /// Name of the wallet the miner pays, filled in per request.
    wallet: String,
}

struct App {
    network: Network,
    dir: PathBuf,
    token: String,
    settings: Mutex<Settings>,
    wallet: Mutex<Option<Wallet>>,
    busy: Mutex<String>,
    node: Mutex<Option<Child>>,
    miner: Mutex<Option<Child>>,
    miner_stats: Arc<Mutex<MinerStats>>,
    /// Unpaid rewards of this wallet's owner, refreshed with every sync.
    account: Mutex<Option<krypton_light::client::MinerAccount>>,
    last_error: Mutex<String>,
    quit: tokio::sync::Notify,
    /// A newer signed release, if one was found.
    update: Mutex<Option<update::Available>>,
    update_status: Mutex<String>,
    /// All wallets of this network; `wallet` above holds the active one when unlocked.
    vault: vault::Vault,
    /// Last "where does my short address resolve" check: (short, when, result).
    published: Mutex<Option<(String, std::time::Instant, Value)>>,
}

impl App {
    /// File of the active wallet.
    fn wallet_path(&self) -> PathBuf {
        let id = self.vault.active_id();
        self.vault
            .path(&self.dir, &id)
            .unwrap_or_else(|| self.dir.join("wallet.krypton"))
    }
    fn settings_path(&self) -> PathBuf {
        self.dir.join("wallet-settings.json")
    }
    fn local_node(&self) -> String {
        format!("http://127.0.0.1:{}", self.network.params().rpc_port)
    }
    fn sources(&self) -> Result<Vec<NodeClient>> {
        let s = self.settings.lock().clone();
        let urls = if s.mode == "full" {
            vec![self.local_node()]
        } else {
            s.nodes
        };
        urls.iter().map(|u| NodeClient::new(u)).collect()
    }
    fn miner_running(&self) -> bool {
        self.miner
            .lock()
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none())
    }
    /// Every node a short address should resolve on: the public node(s), the configured
    /// light-mode nodes and this computer's own node when it runs.
    fn publish_targets(&self) -> Vec<String> {
        let s = self.settings.lock().clone();
        let mut urls: Vec<String> = Vec::new();
        if self.network == Network::Mainnet {
            urls.push(PUBLIC_NODE.into());
        }
        urls.extend(s.nodes.iter().map(|u| u.trim().trim_end_matches('/').to_string()));
        let own = self.node.lock().as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_none());
        if s.mode == "full" || own {
            urls.push(self.local_node());
        }
        let mut seen = std::collections::HashSet::new();
        urls.retain(|u| !u.is_empty() && seen.insert(u.clone()));
        urls
    }
    fn node_label(&self, url: &str) -> String {
        if url == self.local_node() {
            "your node".into()
        } else {
            url.trim_start_matches("https://").trim_start_matches("http://").to_string()
        }
    }
    fn save_settings(&self) -> Result<()> {
        std::fs::write(
            self.settings_path(),
            serde_json::to_vec_pretty(&*self.settings.lock())?,
        )?;
        Ok(())
    }
}

struct Multi(Vec<NodeClient>);
impl ChainSource for Multi {
    fn tip_height(&self) -> Result<u64> {
        let mut last = Err(anyhow!("no node reachable"));
        for c in &self.0 {
            match c.tip_height() {
                Ok(h) => return Ok(h),
                Err(e) => last = Err(e),
            }
        }
        last
    }
    fn header(&self, h: u64) -> Result<krypton_core::BlockHeader> {
        ChainSource::header(self.first()?, h)
    }
    fn compact_blocks(&self, f: u64, c: u32) -> Result<Vec<krypton_light::CompactBlock>> {
        ChainSource::compact_blocks(self.first()?, f, c)
    }
    fn frontier(&self, h: u64) -> Result<krypton_core::tree::Frontier> {
        ChainSource::frontier(self.first()?, h)
    }
    fn witness(&self, p: u64) -> Result<(Vec<krypton_crypto::Digest>, krypton_crypto::Digest)> {
        ChainSource::witness(self.first()?, p)
    }
    fn verified_tip(
        &self,
        params: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        known: Option<&krypton_core::BlockHeader>,
    ) -> Result<Option<krypton_core::BlockHeader>> {
        ChainSource::verified_tip(self.first()?, params, pow, known)
    }
    fn header_in(
        &self,
        h: &krypton_core::BlockHeader,
        tip: &krypton_core::BlockHeader,
    ) -> Result<bool> {
        ChainSource::header_in(self.first()?, h, tip)
    }
    fn submit(&self, tx: &krypton_core::Transaction) -> Result<krypton_core::Hash32> {
        let mut ok = None;
        let mut err = None;
        for c in &self.0 {
            match ChainSource::submit(c, tx) {
                Ok(id) => ok = Some(id),
                Err(e) => err = Some(e),
            }
        }
        ok.ok_or_else(|| err.unwrap_or_else(|| anyhow!("no node")))
    }
}
impl Multi {
    /// With several nodes configured, refuses to continue when they disagree about a
    /// recent common block (an attack or a network split).
    fn cross_check(&self) -> Result<()> {
        if self.0.len() < 2 {
            return Ok(());
        }
        let heights: Vec<(&NodeClient, u64)> = self
            .0
            .iter()
            .filter_map(|c| c.info().ok().map(|i| (c, i.height)))
            .collect();
        let Some(min) = heights.iter().map(|(_, h)| *h).min() else {
            return Ok(());
        };
        let at = min.saturating_sub(2);
        let hashes: std::collections::HashSet<_> = heights
            .iter()
            .filter_map(|(c, _)| NodeClient::header(c, at).ok().map(|h| h.hash()))
            .collect();
        anyhow::ensure!(
            hashes.len() <= 1,
            "configured nodes disagree about block {at}: possible attack or network split"
        );
        Ok(())
    }
    fn first(&self) -> Result<&NodeClient> {
        for c in &self.0 {
            if c.info().is_ok() {
                return Ok(c);
            }
        }
        self.0.first().context("no node configured")
    }
}

type S = State<Arc<App>>;

#[allow(clippy::result_large_err)]
fn check(app: &App, h: &HeaderMap) -> Result<(), Response> {
    if h.get("x-token").and_then(|v| v.to_str().ok()) == Some(app.token.as_str()) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "missing session token").into_response())
    }
}

fn err(e: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": e.to_string()})),
    )
        .into_response()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("{e}"))?
}

fn sibling(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    dir.join(file)
}

async fn state(State(app): S, h: HeaderMap) -> Response {
    if let Err(r) = check(&app, &h) {
        return r;
    }
    let a = app.clone();
    let v = blocking(move || {
        let Some(w) = a.wallet.try_lock() else {
            return Ok(json!({"working": true, "unlocked": true, "exists": true, "busy": a.busy.lock().clone(), "network": a.network.as_str()}));
        };
        let s = a.settings.lock().clone();
        let owner = w
            .as_ref()
            .map(|w| hex::encode(krypton_crypto::field::digest_to_bytes(&w.address().owner)));
        let (address, short, balance, synced, history) = match w.as_ref() {
            Some(w) => (
                Some(w.address().encode()),
                Some(w.address().short()),
                Some(w.balance()),
                w.synced_height(),
                w.history()
                    .iter()
                    .rev()
                    .take(100)
                    .map(|h| {
                        json!({
                            "txid": h.txid, "height": h.height, "amount": h.amount, "fee": h.fee,
                            "memo": h.memo, "to": h.to, "time": h.time, "provable": h.disclosure.is_some(),
                        })
                    })
                    .collect::<Vec<_>>(),
            ),
            None => (None, None, None, None, vec![]),
        };
        drop(w);
        let info = a.sources().ok().and_then(|c| c.first().and_then(|c| c.info().ok()));
        let node_running = a.node.lock().as_mut().map(|c| c.try_wait().ok().flatten().is_none()).unwrap_or(false);
        let miner_running = a.miner.lock().as_mut().map(|c| c.try_wait().ok().flatten().is_none()).unwrap_or(false);
        let mut ms = a.miner_stats.lock().clone();
        ms.running = miner_running;
        if ms.running
            && let Ok(addr) = std::fs::read_to_string(a.dir.join("miner-address.txt"))
            && let Some(name) = vault::name_for_address(&a, &addr)
        {
            ms.wallet = name;
        }
        let reg = a.vault.reg.lock().clone();
        Ok(json!({
            "network": a.network.as_str(),
            "exists": !reg.wallets.is_empty(),
            "unlocked": address.is_some(),
            "active": {
                "id": reg.active,
                "name": reg.get(&reg.active).map(|e| e.name.clone()),
                "locks_in": a.vault.locks_in(&reg.active),
            },
            "wallet_count": reg.wallets.len(),
            "wallet_names": reg.wallets.iter().map(|e| json!({"id": e.id, "name": e.name, "unlocked": a.vault.locks_in(&e.id).is_some()})).collect::<Vec<_>>(),
            "address": address,
            "short": short,
            "owner": owner,
            "balance": balance,
            "synced_height": synced,
            "history": history,
            "settings": s,
            "tip": info,
            "node_running": node_running,
            "miner": ms,
            "rewards": a.account.lock().clone(),
            "payout_interval": a.network.params().payout_interval,
            "busy": a.busy.lock().clone(),
            "error": a.last_error.lock().clone(),
            "maturity": a.network.params().coinbase_maturity,
            "version": env!("CARGO_PKG_VERSION"),
            "update": a.update.lock().clone(),
            "update_status": a.update_status.lock().clone(),
            "genesis": hex::encode(krypton_core::consensus::genesis_block(&a.network.params()).hash()),
        }))
    })
    .await;
    match v {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct CreateReq {
    password: String,
    mnemonic: Option<String>,
    birthday: Option<u64>,
}

/// First-run wallet creation (onboarding); later wallets come from the Wallets tab.
async fn create(State(app): S, h: HeaderMap, Json(r): Json<CreateReq>) -> Response {
    let n = app.vault.reg.lock().wallets.len();
    let name = if n == 0 {
        "Main wallet".to_string()
    } else {
        format!("Wallet {}", n + 1)
    };
    vault::add(
        State(app),
        h,
        Json(vault::AddReq {
            name,
            password: r.password,
            mnemonic: r.mnemonic,
            birthday: r.birthday,
        }),
    )
    .await
}

#[derive(Deserialize)]
struct PassReq {
    password: String,
}

async fn unlock(State(app): S, h: HeaderMap, Json(r): Json<PassReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let w = Wallet::open(&a.wallet_path(), &r.password)?;
        let id = a.vault.active_id();
        vault::remember_fp(&a, &id, &w);
        a.vault.note(&id, &w);
        a.vault
            .unlocked
            .lock()
            .insert(id, vault::Since::now());
        *a.wallet.lock() = Some(w);
        Ok(())
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

async fn reveal(State(app): S, h: HeaderMap, Json(r): Json<PassReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        Ok(Wallet::open(&a.wallet_path(), &r.password)?
            .mnemonic()
            .to_string())
    })
    .await
    {
        Ok(m) => Json(json!({"mnemonic": m})).into_response(),
        Err(e) => err(e),
    }
}

async fn lock(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let id = app.vault.active_id();
    vault::lock_wallet(&app, &id);
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct SendReq {
    to: String,
    amount: String,
    memo: Option<String>,
}

async fn send(State(app): S, h: HeaderMap, Json(r): Json<SendReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let src = Multi(a.sources()?);
        src.cross_check()?;
        let to = r.to.trim().to_string();
        let full = if to.contains("s1") && to.len() < 120 {
            src.first()?.resolve_short(&to)?
        } else {
            to
        };
        let addr = Address::decode(&full)?;
        let amount = parse_amount(&r.amount).context("invalid amount")?;
        let mut guard = a.wallet.lock();
        let w = guard.as_mut().context("wallet locked")?;
        *a.busy.lock() = "Proving a zero-knowledge transfer...".into();
        let busy = &a.busy;
        let res = w.send_auto(
            &src,
            &addr,
            amount,
            DEFAULT_FEE,
            r.memo.as_deref().unwrap_or(""),
            |s| *busy.lock() = s.to_string(),
        );
        *a.busy.lock() = String::new();
        Ok(hex::encode(res?))
    })
    .await
    {
        Ok(id) => Json(json!({"txid": id})).into_response(),
        Err(e) => err(e),
    }
}

async fn publish(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let addr = active_address(&a)?;
        let results = for_each_target(&a, |c| c.publish_address(&addr).map(|_| true));
        let ok = results.iter().filter(|r| r["ok"] == true).count();
        if ok == 0 {
            let why = results
                .iter()
                .map(|r| format!("{}: {}", r["node"].as_str().unwrap_or("?"), r["error"].as_str().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!("could not publish on any node ({why})");
        }
        *a.published.lock() = None;
        Ok(json!({"published": ok, "nodes": results}))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

/// Where the active wallet's short address resolves (cached for ten minutes).
async fn published(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let short = {
            let g = a
                .wallet
                .try_lock_for(std::time::Duration::from_secs(3))
                .context("the wallet is busy; try again")?;
            g.as_ref().context("unlock the wallet first")?.address().short()
        };
        if let Some((s, t, v)) = a.published.lock().as_ref()
            && *s == short
            && t.elapsed() < std::time::Duration::from_secs(600)
        {
            return Ok(v.clone());
        }
        let nodes = for_each_target(&a, |c| c.resolves(&short));
        let v = json!({"short": short, "nodes": nodes});
        *a.published.lock() = Some((short, std::time::Instant::now(), v.clone()));
        Ok(v)
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

fn active_address(a: &App) -> Result<String> {
    let g = a
        .wallet
        .try_lock_for(std::time::Duration::from_secs(5))
        .context("the wallet is busy syncing; try again in a few seconds")?;
    Ok(g.as_ref().context("unlock the wallet first")?.address().encode())
}

/// Runs `f` against every publish target in parallel: `[{node, ok, error}]`.
fn for_each_target(a: &App, f: impl Fn(&NodeClient) -> Result<bool> + Sync) -> Vec<Value> {
    let targets = a.publish_targets();
    std::thread::scope(|s| {
        let jobs: Vec<_> = targets
            .iter()
            .map(|u| {
                let f = &f;
                s.spawn(move || NodeClient::new(u).and_then(|c| f(&c)))
            })
            .collect();
        targets
            .iter()
            .zip(jobs)
            .map(|(u, j)| {
                let label = a.node_label(u);
                match j.join().unwrap_or_else(|_| Err(anyhow!("internal error"))) {
                    Ok(ok) => json!({"node": label, "ok": ok}),
                    Err(e) => json!({"node": label, "ok": false, "error": format!("{e:#}")}),
                }
            })
            .collect()
    })
}

/// The latest blocks from the first data source, for the overview.
async fn recent_blocks(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        a.sources()?
            .into_iter()
            .next()
            .context("no node configured")?
            .blocks(14)
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

async fn save_settings(State(app): S, h: HeaderMap, Json(s): Json<Settings>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    if s.mode != "light" && s.mode != "full" {
        return err("mode must be light or full");
    }
    *app.settings.lock() = s;
    match app.save_settings() {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

async fn node_start(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    match spawn_node(&app) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

/// Starts our own full node unless it already runs.
fn spawn_node(app: &App) -> Result<()> {
    let mut n = app.node.lock();
    if n.as_mut()
        .is_some_and(|c| c.try_wait().ok().flatten().is_none())
    {
        return Ok(());
    }
    let c = hidden(&mut Command::new(sibling("kryptond")))
        .args(["--network", app.network.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("could not start kryptond: {e}"))?;
    *n = Some(c);
    Ok(())
}

async fn node_stop(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    if let Some(mut c) = app.node.lock().take() {
        let _ = c.kill();
    }
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct MinerReq {
    backend: String,
    max_cpu: Option<u32>,
}

async fn miner_start(State(app): S, h: HeaderMap, Json(r): Json<MinerReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let addr = match app.wallet.lock().as_ref() {
        Some(w) => w.address().encode(),
        None => return err("unlock the wallet first"),
    };
    match spawn_miner(&app, &addr, &r.backend, r.max_cpu.unwrap_or(50)) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

/// Starts krypton-miner paying `addr` and follows its JSON status lines.
fn spawn_miner(app: &App, addr: &str, backend: &str, max_cpu: u32) -> Result<()> {
    if !["auto", "cpu", "cuda", "opencl"].contains(&backend) {
        anyhow::bail!("unknown backend");
    }
    let node = {
        let s = app.settings.lock();
        if s.mode == "full" {
            app.local_node()
        } else {
            s.nodes.first().cloned().unwrap_or_else(|| app.local_node())
        }
    };
    if let Some(mut c) = app.miner.lock().take() {
        let _ = c.kill();
    }
    let addr_file = app.dir.join("miner-address.txt");
    std::fs::write(&addr_file, addr)?;
    std::fs::write(
        app.dir.join("miner-settings.json"),
        json!({"backend": backend, "max_cpu": max_cpu}).to_string(),
    )?;
    let mut cmd = Command::new(sibling("krypton-miner"));
    hidden(&mut cmd);
    cmd.arg("--node")
        .arg(&node)
        .arg("--address-file")
        .arg(&addr_file)
        .arg("--backend")
        .arg(backend)
        .arg("--json");
    cmd.arg("--max-cpu").arg(max_cpu.clamp(1, 100).to_string());
    match cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(mut c) => {
            let out = c.stdout.take();
            *app.miner_stats.lock() = MinerStats {
                running: true,
                backend: backend.to_string(),
                ..Default::default()
            };
            let stats = app.miner_stats.clone();
            std::thread::spawn(move || {
                if let Some(o) = out {
                    for line in std::io::BufReader::new(o).lines().map_while(|l| l.ok()) {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            let mut s = stats.lock();
                            match v["event"].as_str() {
                                Some("status") => {
                                    s.hashrate = v["hashrate_hs"].as_f64().unwrap_or(0.0);
                                    s.blocks = v["accepted_blocks"].as_u64().unwrap_or(0);
                                    s.shares = v["accepted_shares"].as_u64().unwrap_or(0);
                                    s.device = v["device"].as_str().unwrap_or("").to_string();
                                    s.network_fraction =
                                        v["network_fraction"].as_f64().unwrap_or(0.0);
                                    s.expected_per_day =
                                        v["expected_gnr_per_day"].as_f64().unwrap_or(0.0);
                                }
                                Some("block") => {
                                    s.blocks += 1;
                                    s.shares += 1;
                                    s.last = format!("block {} found", v["height"]);
                                }
                                Some("share") => {
                                    s.shares += 1;
                                    s.last = format!("share at height {}", v["height"]);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            });
            *app.miner.lock() = Some(c);
            Ok(())
        }
        Err(e) => Err(anyhow!("could not start krypton-miner: {e}")),
    }
}

async fn miner_stop(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    if let Some(mut c) = app.miner.lock().take() {
        let _ = c.kill();
    }
    app.miner_stats.lock().running = false;
    Json(json!({"ok": true})).into_response()
}

async fn devices(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    match blocking(|| {
        Ok(hidden(&mut Command::new(sibling("krypton-miner")))
            .arg("--list-devices")
            .output()?)
    })
    .await
    {
        Ok(o) => Json(json!({"devices": String::from_utf8_lossy(&o.stdout)})).into_response(),
        Err(e) => err(e),
    }
}

fn data_dir(network: Network) -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("GenorynKrypton"))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".genoryn-krypton")))
        .unwrap_or_else(|| PathBuf::from(".genoryn-krypton"));
    base.join(network.as_str())
}

/// Child processes (node, miner, browser launcher) never open console windows.
fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Downloads and verifies the newer release, then hands over to its installer, which
/// replaces the programs and starts the wallet again. A running miner resumes.
async fn install_update(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let Some(av) = app.update.lock().clone() else {
        return err("no update available");
    };
    if !av.can_install {
        open_browser(&av.page);
        return Json(json!({"ok": true, "opened": av.page})).into_response();
    }
    *app.update_status.lock() = format!("Downloading {} and checking its signature…", av.version);
    let a = app.clone();
    let r = blocking(move || {
        let dir = std::env::temp_dir().join(format!("genoryn-update-{}", av.version));
        let installer = update::download_verified(&av, &dir)?;
        let mining = a
            .miner
            .lock()
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none());
        if mining && let Ok(s) = std::fs::read(a.dir.join("miner-settings.json")) {
            std::fs::write(a.dir.join("resume-mining.json"), s)?;
        }
        let install_dir = std::env::current_exe()?
            .parent()
            .context("install directory")?
            .to_path_buf();
        hidden(&mut Command::new(&installer))
            .arg("--silent")
            .arg("--launch")
            .arg("--dir")
            .arg(&install_dir)
            .spawn()
            .context("start the installer")?;
        Ok(())
    })
    .await;
    match r {
        Ok(()) => {
            *app.update_status.lock() =
                "Installing the update; the wallet restarts in a moment".into();
            let q = app.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                q.quit.notify_one();
            });
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => {
            *app.update_status.lock() = format!("Update failed: {e}");
            err(e)
        }
    }
}

#[derive(Deserialize)]
struct RangeQ {
    range: Option<String>,
}

async fn net_stats(
    State(app): S,
    h: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<RangeQ>,
) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let range = q.range.unwrap_or_else(|| "24h".into());
    if !["1h", "6h", "24h", "7d", "30d", "all"].contains(&range.as_str()) {
        return err("unknown range");
    }
    let a = app.clone();
    match blocking(move || {
        let src = a
            .sources()?
            .into_iter()
            .next()
            .context("no node configured")?;
        src.stats(&range)
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

async fn payment_proof(State(app): S, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let txid = b["txid"].as_str().unwrap_or("").to_string();
    let a = app.clone();
    match blocking(move || {
        let g = a.wallet.lock();
        let w = g.as_ref().context("unlock the wallet first")?;
        Ok(json!({ "proof": w.payment_proof(&txid)? }))
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

async fn verify_proof(State(app): S, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let text = b["proof"].as_str().unwrap_or("").trim().to_string();
    let a = app.clone();
    match blocking(move || {
        let src = a
            .sources()?
            .into_iter()
            .next()
            .context("no node configured")?;
        let g = a.wallet.lock();
        let w = g.as_ref().context("unlock the wallet first")?;
        Ok(serde_json::to_value(w.verify_payment(&src, &text)?)?)
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

/// Opens a fixed project page in the system browser; the UI never passes a raw URL.
async fn open_link(State(app): S, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let own;
    let url = match b["target"].as_str().unwrap_or("") {
        "address" => {
            let Some(short) = app.wallet.try_lock().and_then(|g| g.as_ref().map(|w| w.address().short())) else {
                return err("unlock the wallet first");
            };
            own = format!("https://genoryn.io/explorer#/address/{short}");
            own.as_str()
        }
        "explorer" => "https://genoryn.io/explorer",
        "website" => "https://genoryn.io/",
        "docs" => "https://genoryn.io/docs",
        "source" => "https://github.com/genoryn/genoryn",
        "releases" => "https://github.com/genoryn/genoryn/releases",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "unknown link"})),
            )
                .into_response();
        }
    };
    open_browser(url);
    Json(json!({"ok": true})).into_response()
}

async fn quit(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    app.quit.notify_one();
    Json(json!({"ok": true})).into_response()
}

/// If a wallet is already running for this network, returns its URL.
fn running_instance(dir: &std::path::Path) -> Option<String> {
    use std::io::{Read, Write};
    let url = std::fs::read_to_string(dir.join("wallet-instance.url")).ok()?;
    let host = url
        .trim()
        .strip_prefix("http://")?
        .split('/')
        .next()?
        .to_string();
    let addr: std::net::SocketAddr = host.parse().ok()?;
    let mut s =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(600)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_millis(800)))
        .ok()?;
    write!(s, "GET /api/ping HTTP/1.0\r\nHost: {host}\r\n\r\n").ok()?;
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    buf.contains("genoryn-krypton-wallet")
        .then(|| url.trim().to_string())
}

/// Links the embedded window may hand to the system browser.
fn project_link(url: &str) -> bool {
    (url.starts_with("https://genoryn.io/") || url.starts_with("https://github.com/genoryn/"))
        && url
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~:/?#=+".contains(&c))
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    let _ = hidden(Command::new("rundll32").args(["url.dll,FileProtocolHandler", url])).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

/// Runs the local wallet server. Sends (url, app) once listening; returns after shutdown.
async fn serve(
    port: u16,
    network: Network,
    dir: PathBuf,
    ready: std::sync::mpsc::Sender<Result<(String, Arc<App>)>>,
) -> Result<()> {
    let settings: Settings = std::fs::read(dir.join("wallet-settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Settings {
            mode: "light".into(),
            nodes: vec![PUBLIC_NODE.into()],
        });
    let token = hex::encode(rand::random::<[u8; 24]>());
    let vault = vault::Vault::new(&dir);
    let app = Arc::new(App {
        network,
        dir,
        token: token.clone(),
        settings: Mutex::new(settings),
        wallet: Mutex::new(None),
        busy: Mutex::new(String::new()),
        node: Mutex::new(None),
        miner: Mutex::new(None),
        miner_stats: Arc::new(Mutex::new(MinerStats::default())),
        account: Mutex::new(None),
        last_error: Mutex::new(String::new()),
        quit: tokio::sync::Notify::new(),
        update: Mutex::new(None),
        update_status: Mutex::new(String::new()),
        vault,
        published: Mutex::new(None),
    });

    // Full-node mode: our node starts with the wallet.
    if app.settings.lock().mode == "full" {
        let _ = spawn_node(&app);
    }
    // After an in-app update, resume the miner that was running before it.
    let resume = app.dir.join("resume-mining.json");
    if let Ok(b) = std::fs::read(&resume) {
        let _ = std::fs::remove_file(&resume);
        let a = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            let v: Value = serde_json::from_slice(&b).unwrap_or_default();
            if let Ok(addr) = std::fs::read_to_string(a.dir.join("miner-address.txt")) {
                let backend = v["backend"].as_str().unwrap_or("auto").to_string();
                let max_cpu = v["max_cpu"].as_u64().unwrap_or(50) as u32;
                let _ = spawn_miner(&a, addr.trim(), &backend, max_cpu);
            }
        });
    }
    // Look for a newer signed release now and every six hours.
    let up = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            if let Ok(Ok(found)) = tokio::task::spawn_blocking(update::check).await {
                *up.update.lock() = found;
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });

    // Background sync while unlocked.
    let bg = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let a = bg.clone();
            let _ = tokio::task::spawn_blocking(move || {
                vault::expire(&a);
                let Ok(sources) = a.sources() else { return };
                let src = Multi(sources);
                // Lock order everywhere: registry, then active wallet, then others.
                let active_id = a.vault.active_id();
                if let Some(mut g) = a.wallet.try_lock()
                    && let Some(w) = g.as_mut()
                {
                    match src.cross_check().and_then(|_| w.sync(&src, |_, _| {})) {
                        Ok(_) => a.last_error.lock().clear(),
                        Err(e) => *a.last_error.lock() = format!("sync: {e}"),
                    }
                    a.vault.note(&active_id, w);
                    let owner = krypton_crypto::field::digest_to_bytes(&w.address().owner);
                    drop(g);
                    if let Some(c) = src.0.first() {
                        *a.account.lock() = c.miner_account(&owner).ok();
                    }
                }
                vault::sync_others(&a, &src);
            })
            .await;
        }
    });

    let page: &'static str = Box::leak(
        UI.replace("<!--FX_HEAD-->", assets::FX_HEAD)
            .into_boxed_str(),
    );
    let router = Router::new()
        .route(
            "/",
            get(move || async move { ([(header::CACHE_CONTROL, "no-store")], Html(page)) }),
        )
        .route("/assets/{*path}", get(asset))
        .route("/api/state", get(state))
        .route("/api/create", post(create))
        .route("/api/unlock", post(unlock))
        .route("/api/wallets", get(vault::list))
        .route("/api/wallets/add", post(vault::add))
        .route("/api/wallets/unlock", post(vault::unlock_one))
        .route("/api/wallets/lock", post(vault::lock_one))
        .route("/api/wallets/lock-all", post(vault::lock_all))
        .route("/api/wallets/use", post(vault::use_wallet))
        .route("/api/wallets/rename", post(vault::rename))
        .route("/api/wallets/remove", post(vault::remove))
        .route("/api/lock", post(lock))
        .route("/api/reveal", post(reveal))
        .route("/api/send", post(send))
        .route("/api/publish", post(publish))
        .route("/api/settings", post(save_settings))
        .route("/api/node/start", post(node_start))
        .route("/api/node/stop", post(node_stop))
        .route("/api/miner/start", post(miner_start))
        .route("/api/miner/stop", post(miner_stop))
        .route("/api/devices", get(devices))
        .route("/api/ping", get(|| async { "genoryn-krypton-wallet" }))
        .route("/api/quit", post(quit))
        .route("/api/open", post(open_link))
        .route("/api/proof", post(payment_proof))
        .route("/api/stats", get(net_stats))
        .route("/api/blocks", get(recent_blocks))
        .route("/api/published", get(published))
        .route("/api/update", post(install_update))
        .route("/api/verify-proof", post(verify_proof))
        .with_state(app.clone());
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("cannot listen on 127.0.0.1:{port}: {e}")));
            return Err(e.into());
        }
    };
    let url = format!("http://{}/#{}", listener.local_addr()?, token);
    let instance_file = app.dir.join("wallet-instance.url");
    let _ = std::fs::write(&instance_file, &url);
    println!(
        "Genoryn Krypton wallet ({}) running at:\n{url}",
        network.as_str()
    );
    let _ = ready.send(Ok((url, app.clone())));
    let q = app.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = q.quit.notified() => {},
            }
        })
        .await?;
    let _ = std::fs::remove_file(&instance_file);
    if let Some(mut c) = app.miner.lock().take() {
        let _ = c.kill();
    }
    if let Some(mut c) = app.node.lock().take() {
        let _ = c.kill();
    }
    Ok(())
}

fn main() -> Result<()> {
    let a = Args::parse();
    let network = Network::parse(&a.network).context("unknown network")?;
    let dir = data_dir(network);
    std::fs::create_dir_all(&dir)?;
    // Second launch (desktop icon clicked again): show the running wallet instead.
    if let Some(url) = running_instance(&dir) {
        if !a.no_browser {
            window::show(&url, None, None);
        }
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let port = a.port;
    let server = std::thread::spawn(move || -> Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(serve(port, network, dir, tx))
    });
    let (url, app) = rx.recv().context("wallet server stopped")??;
    if a.no_browser {
        let _ = server.join();
        return Ok(());
    }
    window::show(&url, Some(app), Some(server));
    Ok(())
}

/// The wallet window: a native window with an embedded WebView2 view on Windows,
/// the system browser elsewhere (or when WebView2 is missing).
mod window {
    use super::App;
    use std::sync::Arc;

    type Server = std::thread::JoinHandle<anyhow::Result<()>>;

    #[cfg(windows)]
    pub fn show(url: &str, app: Option<Arc<App>>, server: Option<Server>) {
        use tao::dpi::LogicalSize;
        use tao::event::{Event, WindowEvent};
        use tao::event_loop::{ControlFlow, EventLoopBuilder};
        use tao::window::{Icon, WindowBuilder};

        #[derive(Debug)]
        enum User {
            Quit,
        }
        let event_loop = EventLoopBuilder::<User>::with_user_event().build();
        let icon = decode_icon().and_then(|(rgba, w, h)| Icon::from_rgba(rgba, w, h).ok());
        let Ok(window) = WindowBuilder::new()
            .with_title("Genoryn Krypton")
            .with_inner_size(LogicalSize::new(1280.0, 860.0))
            .with_min_inner_size(LogicalSize::new(420.0, 600.0))
            .with_window_icon(icon)
            .with_theme(Some(tao::window::Theme::Dark))
            .build(&event_loop)
        else {
            return fallback(url, server);
        };
        let origin = url.split('#').next().unwrap_or(url).to_string();
        let webview = wry::WebViewBuilder::new()
            .with_url(url)
            .with_background_color((0, 0, 0, 255))
            .with_devtools(false)
            .with_navigation_handler(move |to| {
                if to.starts_with(&origin) || to.starts_with("about:") {
                    return true;
                }
                if super::project_link(&to) {
                    super::open_browser(&to);
                }
                false
            })
            .with_new_window_req_handler(|to, _| {
                if super::project_link(&to) {
                    super::open_browser(&to);
                }
                wry::NewWindowResponse::Deny
            })
            .build(&window);
        let Ok(webview) = webview else {
            drop(window);
            return fallback(url, server);
        };
        // When the server stops (Quit button), close the window too.
        let proxy = event_loop.create_proxy();
        let mut server = server;
        let watcher = server.take().map(|s| {
            std::thread::spawn(move || {
                let _ = s.join();
                let _ = proxy.send_event(User::Quit);
            })
        });
        let _keep = (webview, watcher);
        event_loop.run(move |event, _, flow| {
            *flow = ControlFlow::Wait;
            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => match &app {
                    // While mining, closing the window keeps everything running in the
                    // background (the desktop icon reopens it; Quit stops it). Otherwise stop
                    // the server (and its node); the watcher then posts Quit.
                    Some(a) => {
                        window.set_visible(false);
                        if !a.miner_running() {
                            a.quit.notify_one();
                        }
                    }
                    None => *flow = ControlFlow::Exit,
                },
                Event::UserEvent(User::Quit) => *flow = ControlFlow::Exit,
                _ => {}
            }
        });
    }

    #[cfg(windows)]
    fn decode_icon() -> Option<(Vec<u8>, u32, u32)> {
        let mut dec = png::Decoder::new(&include_bytes!("../assets/mark.png")[..]);
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut r = dec.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).ok()?;
        buf.truncate(info.buffer_size());
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .chunks(3)
                .flat_map(|c| [c[0], c[1], c[2], 255])
                .collect(),
            _ => return None,
        };
        Some((rgba, info.width, info.height))
    }

    #[cfg(not(windows))]
    pub fn show(url: &str, _app: Option<Arc<App>>, server: Option<Server>) {
        fallback(url, server)
    }

    fn fallback(url: &str, server: Option<Server>) {
        super::open_browser(url);
        if let Some(s) = server {
            let _ = s.join();
        }
    }
}
