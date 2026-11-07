//! Several wallets side by side.
//!
//! A registry (`wallets.json`) lists the wallet files; each file is encrypted with its
//! own password. One wallet is *active* (it sends, receives and is shown on the Wallet
//! tab); others may be unlocked too so the app can show a total. Every unlock lasts at
//! most 24 hours, then the wallet locks and needs its password again. Locking never
//! stops a running miner, which only needs a public address.

use super::*;
use anyhow::{bail, ensure};
use krypton_wallet_core::Balance;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

pub const UNLOCK_TTL: Duration = Duration::from_secs(24 * 3600);

/// When a wallet was unlocked. The monotonic clock may stop while the computer sleeps,
/// so the wall clock is kept too and whichever shows more time wins.
#[derive(Clone, Copy)]
pub struct Since {
    mono: Instant,
    wall: SystemTime,
}

impl Since {
    pub fn now() -> Self {
        Since {
            mono: Instant::now(),
            wall: SystemTime::now(),
        }
    }
    pub fn elapsed(&self) -> Duration {
        let wall = self.wall.elapsed().unwrap_or(Duration::ZERO);
        self.mono.elapsed().max(wall)
    }
}
const REGISTRY: &str = "wallets.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// Path relative to the network directory.
    pub file: String,
    #[serde(default)]
    pub created: u64,
    /// Truncated hash of the address, so a phrase is not added twice even while its
    /// wallet is locked.
    #[serde(default)]
    pub fp: String,
    /// The wallet's public receiving address, so payments between this app's own wallets
    /// work even while the recipient is locked and its short form is unpublished.
    #[serde(default)]
    pub addr: String,
}

/// Stores the public address of the registered wallet with fingerprint `fp`.
pub fn remember_addr(app: &App, fp: &str, addr: &str) {
    let mut reg = app.vault.reg.lock();
    if let Some(e) = reg.wallets.iter_mut().find(|e| e.fp == fp)
        && e.addr != addr
    {
        e.addr = addr.to_string();
        let _ = reg.save(&app.dir);
    }
}

/// The full address of one of this app's wallets whose short form has id `id`.
pub fn own_address(app: &App, id: &[u8]) -> Option<String> {
    let reg = app.vault.reg.lock().clone();
    reg.wallets
        .iter()
        .filter(|e| !e.addr.is_empty())
        .find(|e| Address::decode(&e.addr).is_ok_and(|a| a.short_id().as_slice() == id))
        .map(|e| e.addr.clone())
}

pub fn fingerprint(w: &Wallet) -> String {
    address_fingerprint(&w.address().encode())
}

fn address_fingerprint(encoded: &str) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(format!("Krypton/wallet-fingerprint/{}", encoded.trim()));
    hex::encode(&h[..12])
}

/// Name of the registered wallet with this encoded address, if known.
pub fn name_for_address(app: &App, encoded: &str) -> Option<String> {
    let fp = address_fingerprint(encoded);
    app.vault
        .reg
        .lock()
        .wallets
        .iter()
        .find(|e| e.fp == fp)
        .map(|e| e.name.clone())
}

/// Records the fingerprint and public address of a wallet that predates them
/// (e.g. a migrated one).
pub fn remember_fp(app: &App, id: &str, w: &Wallet) {
    let fp = fingerprint(w);
    let addr = w.address().encode();
    let mut reg = app.vault.reg.lock();
    if let Some(e) = reg.wallets.iter_mut().find(|e| e.id == id)
        && (e.fp != fp || e.addr != addr)
    {
        e.fp = fp;
        e.addr = addr;
        let _ = reg.save(&app.dir);
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub active: String,
    pub wallets: Vec<Entry>,
}

impl Registry {
    /// Loads the registry; an installation from before multi-wallet support becomes a
    /// registry with its single wallet, unchanged on disk.
    pub fn load(dir: &Path) -> Self {
        if let Ok(b) = std::fs::read(dir.join(REGISTRY))
            && let Ok(r) = serde_json::from_slice::<Registry>(&b)
        {
            return r;
        }
        if dir.join("wallet.krypton").exists() {
            return Registry {
                active: "main".into(),
                wallets: vec![Entry {
                    id: "main".into(),
                    name: "Main wallet".into(),
                    file: "wallet.krypton".into(),
                    created: 0,
                    fp: String::new(),
                    addr: String::new(),
                }],
            };
        }
        Registry::default()
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        let tmp = dir.join(format!("{REGISTRY}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, dir.join(REGISTRY))?;
        Ok(())
    }
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.wallets.iter().find(|e| e.id == id)
    }
}

#[derive(Clone, Serialize)]
pub struct Cached {
    pub balance: Balance,
    pub short: String,
    pub synced: Option<u64>,
    /// Payments to this wallet waiting in the mempool.
    pub incoming: u64,
}

#[derive(Default)]
pub struct Vault {
    pub reg: Mutex<Registry>,
    /// Unlocked wallets other than the active one.
    pub others: Mutex<HashMap<String, Wallet>>,
    /// When each unlocked wallet (active included) was unlocked.
    pub unlocked: Mutex<HashMap<String, Since>>,
    /// Last known balance of every wallet seen unlocked during this session.
    pub cache: Mutex<HashMap<String, Cached>>,
    /// Wallets temporarily taken out of `others` by the background sync.
    pub syncing: Mutex<HashSet<String>>,
    /// Wallets with a swap in progress or an open offer (closing the window keeps the app
    /// running for them). The 24-hour lock applies to them as to any other.
    pub hold: Mutex<HashSet<String>>,
}

impl Vault {
    pub fn new(dir: &Path) -> Self {
        Vault {
            reg: Mutex::new(Registry::load(dir)),
            ..Default::default()
        }
    }
    pub fn path(&self, dir: &Path, id: &str) -> Option<PathBuf> {
        self.reg.lock().get(id).map(|e| dir.join(&e.file))
    }
    pub fn active_id(&self) -> String {
        self.reg.lock().active.clone()
    }
    pub fn note(&self, id: &str, w: &Wallet) {
        self.cache.lock().insert(
            id.to_string(),
            Cached {
                balance: w.balance(),
                short: w.address().short(),
                synced: w.synced_height(),
                incoming: w.incoming().iter().map(|i| i.value).sum(),
            },
        );
    }
    pub fn set_hold(&self, id: &str, busy: bool) {
        let mut h = self.hold.lock();
        if busy {
            h.insert(id.to_string());
        } else {
            h.remove(id);
        }
    }
    pub fn locks_in(&self, id: &str) -> Option<u64> {
        self.unlocked
            .lock()
            .get(id)
            .map(|t| UNLOCK_TTL.saturating_sub(t.elapsed()).as_secs())
    }
}

fn valid_name(n: &str) -> Result<String> {
    let n = n.trim();
    ensure!(!n.is_empty(), "give the wallet a name");
    ensure!(
        n.chars().count() <= 40,
        "use at most 40 characters for the name"
    );
    ensure!(
        !n.chars().any(|c| c.is_control()),
        "the name contains invalid characters"
    );
    Ok(n.to_string())
}

/// Locks one wallet (active or not). The miner keeps running.
pub fn lock_wallet(app: &App, id: &str) {
    if app.vault.active_id() == id {
        // Clear the wallet before dropping its unlock record, so `expired` keeps
        // refusing it while we wait for a running sync to let go of it.
        *app.wallet.lock() = None;
        *app.account.lock() = None;
        app.vault.unlocked.lock().remove(id);
        app.vault.set_hold(id, false);
    } else {
        // `put_back` checks the record, so it goes first here.
        app.vault.unlocked.lock().remove(id);
        app.vault.others.lock().remove(id);
    }
}

/// Makes `id` the active wallet (it may be locked).
pub fn activate(app: &App, id: &str) -> Result<()> {
    let mut reg = app.vault.reg.lock();
    ensure!(reg.get(id).is_some(), "no such wallet");
    if reg.active == id {
        return Ok(());
    }
    let mut active = app
        .wallet
        .try_lock_for(std::time::Duration::from_secs(3))
        .context("the wallet is busy syncing; try again in a few seconds")?;
    let mut others = app.vault.others.lock();
    if let Some(w) = active.take() {
        others.insert(reg.active.clone(), w);
    }
    *active = others.remove(id);
    reg.active = id.to_string();
    reg.save(&app.dir)?;
    *app.account.lock() = None;
    Ok(())
}

/// Returns a wallet the background sync took out of `others`, unless it was locked,
/// removed or made active meanwhile.
pub fn put_back(app: &App, id: &str, w: Wallet) {
    app.vault.syncing.lock().remove(id);
    if !app.vault.unlocked.lock().contains_key(id) || app.vault.reg.lock().get(id).is_none() {
        return;
    }
    if app.vault.active_id() == id {
        let mut a = app.wallet.lock();
        if a.is_none() {
            *a = Some(w);
        }
    } else {
        app.vault.others.lock().insert(id.to_string(), w);
    }
}

/// Whether `id` was unlocked more than 24 hours ago (it must not be used any more,
/// even before the background task gets to lock it).
pub fn expired(app: &App, id: &str) -> bool {
    app.vault
        .unlocked
        .lock()
        .get(id)
        .is_some_and(|t| t.elapsed() >= UNLOCK_TTL)
}

/// Locks every wallet whose 24 hours are over.
pub fn expire(app: &App) {
    let due: Vec<String> = app
        .vault
        .unlocked
        .lock()
        .iter()
        .filter(|(_, t)| t.elapsed() >= UNLOCK_TTL)
        .map(|(id, _)| id.clone())
        .collect();
    for id in due {
        lock_wallet(app, &id);
    }
}

/// Syncs the unlocked non-active wallets, one at a time, without holding locks the UI needs.
pub fn sync_others(app: &App, src: &Multi, pool: Option<&Pool>) {
    let Ok(tip) = src.tip_height() else { return };
    // Pending incoming payments are checked every round; a full sync only after a block.
    if let Some((txs, live)) = pool {
        for (id, w) in app.vault.others.lock().iter_mut() {
            w.scan_mempool(txs, live);
            app.vault.note(id, w);
            crate::snapshot(w);
        }
    }
    let ids: Vec<String> = app
        .vault
        .others
        .lock()
        .iter()
        .filter(|(_, w)| w.synced_height().is_none_or(|h| h < tip))
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        let Some(mut w) = app.vault.others.lock().remove(&id) else {
            continue;
        };
        app.vault.syncing.lock().insert(id.clone());
        let _ = w.sync(src, |_, _| {});
        app.vault.set_hold(&id, crate::swaps::busy(&w));
        app.vault.note(&id, &w);
        remember_fp(app, &id, &w);
        crate::snapshot(&w);
        put_back(app, &id, w);
    }
    // Swaps and offers of every unlocked wallet advance too, not only the active one's
    // (so two wallets of this computer can even trade with each other).
    let txs = pool.map(|p| p.0.as_slice()).unwrap_or(&[]);
    let busy: Vec<String> = app
        .vault
        .others
        .lock()
        .iter()
        .filter(|(_, w)| crate::swaps::has_work(w))
        .map(|(id, _)| id.clone())
        .collect();
    for id in busy {
        let Some(mut w) = app.vault.others.lock().remove(&id) else {
            continue;
        };
        app.vault.syncing.lock().insert(id.clone());
        crate::swaps::tick(app, &mut w, src, txs);
        app.vault.set_hold(&id, crate::swaps::busy(&w));
        crate::snapshot(&w);
        put_back(app, &id, w);
    }
}

/// The quick board round for the other unlocked wallets.
pub fn quick_others(app: &App, src: &Multi) {
    let busy: Vec<String> = app
        .vault
        .others
        .lock()
        .iter()
        .filter(|(_, w)| crate::board::quick_work(w))
        .map(|(id, _)| id.clone())
        .collect();
    for id in busy {
        let Some(mut w) = app.vault.others.lock().remove(&id) else {
            continue;
        };
        app.vault.syncing.lock().insert(id.clone());
        crate::swaps::quick(app, &mut w, src);
        crate::snapshot(&w);
        put_back(app, &id, w);
    }
}

// ---- HTTP handlers ----

pub async fn list(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let reg = app.vault.reg.lock().clone();
    let cache = app.vault.cache.lock().clone();
    let unlocked = app.vault.unlocked.lock().clone();
    let mut total = Balance::default();
    let mut counted = 0;
    let wallets: Vec<Value> = reg
        .wallets
        .iter()
        .map(|e| {
            let c = cache.get(&e.id);
            if let Some(c) = c {
                total.spendable += c.balance.spendable;
                total.pending += c.balance.pending;
                total.immature += c.balance.immature;
                total.total += c.balance.total;
                counted += 1;
            }
            let open = unlocked.contains_key(&e.id);
            json!({
                "id": e.id,
                "name": e.name,
                "active": e.id == reg.active,
                "unlocked": open,
                "locks_in": unlocked.get(&e.id).map(|t| UNLOCK_TTL.saturating_sub(t.elapsed()).as_secs()),
                "balance": c.map(|c| c.balance),
                "balance_is_last_known": c.is_some() && !open,
                "short": c.map(|c| c.short.clone()),
                "synced": c.and_then(|c| c.synced),
                "incoming": c.map(|c| c.incoming).unwrap_or(0),
            })
        })
        .collect();
    Json(json!({
        "active": reg.active,
        "wallets": wallets,
        "total": total,
        "counted": counted,
        "unknown": reg.wallets.len() - counted,
        "unlock_hours": UNLOCK_TTL.as_secs() / 3600,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct AddReq {
    pub name: String,
    pub password: String,
    pub mnemonic: Option<String>,
    pub birthday: Option<u64>,
}

/// Adds a wallet (new or restored) with its own password. The first wallet becomes
/// the active "main" wallet at the classic path.
pub async fn add(State(app): S, h: HeaderMap, Json(r): Json<AddReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let name = valid_name(&r.name)?;
        // The file opens with this password alone: a stolen file can only be attacked by
        // guessing it, so new wallets need a strong one.
        let st = krypton_wallet_core::password::strength(&r.password);
        ensure!(st.ok, "{}", st.hint);
        let restoring = r.mnemonic.as_deref().is_some_and(|m| !m.trim().is_empty());
        let m = if restoring {
            let m = r.mnemonic.as_deref().unwrap_or_default();
            keys::validate_mnemonic(m)?;
            keys::normalize(m)?
        } else {
            keys::new_mnemonic()
        };
        let birthday = if restoring {
            r.birthday.unwrap_or(0)
        } else {
            Multi(a.sources()?).tip_height().unwrap_or(0)
        };
        let first = a.vault.reg.lock().wallets.is_empty();
        let (id, file) = if first {
            ("main".to_string(), "wallet.krypton".to_string())
        } else {
            let reg = a.vault.reg.lock();
            let mut id = hex::encode(rand::random::<[u8; 4]>());
            while reg.get(&id).is_some() {
                id = hex::encode(rand::random::<[u8; 4]>());
            }
            let file = format!("wallets/{id}.krypton");
            (id, file)
        };
        let path = a.dir.join(&file);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let w = Wallet::create(
            &path,
            &r.password,
            &m,
            "",
            a.network,
            birthday,
            Argon2Params::STRONG,
        )?;
        let me = w.address();
        let fp = fingerprint(&w);
        let duplicate = a.vault.reg.lock().wallets.iter().any(|e| e.fp == fp)
            || a.wallet.lock().iter().any(|o| o.address() == me)
            || a.vault.others.lock().values().any(|o| o.address() == me);
        if duplicate {
            let _ = std::fs::remove_file(&path);
            bail!("this recovery phrase is already open as another wallet");
        }
        a.vault.note(&id, &w);
        {
            let mut reg = a.vault.reg.lock();
            reg.wallets.push(Entry {
                id: id.clone(),
                name,
                file,
                created: krypton_core::now(),
                fp,
                addr: me.encode(),
            });
            if first {
                reg.active = id.clone();
            }
            reg.save(&a.dir)?;
        }
        a.vault.unlocked.lock().insert(id.clone(), Since::now());
        if first {
            *a.wallet.lock() = Some(w);
        } else {
            a.vault.others.lock().insert(id.clone(), w);
        }
        Ok(if restoring {
            json!({"id": id})
        } else {
            json!({"id": id, "mnemonic": m.as_str()})
        })
    })
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct IdReq {
    id: String,
    password: Option<String>,
    name: Option<String>,
}

// Secrets from requests are wiped from memory once handled.
impl Drop for AddReq {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
        if let Some(m) = self.mnemonic.as_mut() {
            m.zeroize();
        }
    }
}
impl Drop for IdReq {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(p) = self.password.as_mut() {
            p.zeroize();
        }
    }
}

pub async fn unlock_one(State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let path = a.vault.path(&a.dir, &r.id).context("no such wallet")?;
        let pw = r.password.as_deref().unwrap_or("");
        let w = Wallet::open(&path, pw)?;
        let weak = !krypton_wallet_core::password::strength(pw).ok;
        remember_fp(&a, &r.id, &w);
        a.vault.note(&r.id, &w);
        a.vault.unlocked.lock().insert(r.id.clone(), Since::now());
        if a.vault.active_id() == r.id {
            *a.wallet.lock() = Some(w);
        } else {
            a.vault.others.lock().insert(r.id.clone(), w);
        }
        Ok(weak)
    })
    .await
    {
        Ok(weak) => Json(json!({"ok": true, "weak_password": weak})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct PasswordReq {
    id: String,
    old: String,
    new: String,
}

impl Drop for PasswordReq {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.old.zeroize();
        self.new.zeroize();
    }
}

/// Changes a wallet's password: checks the old one against the file, then re-encrypts
/// the file under the new one with a fresh salt. Works for open and locked wallets.
pub async fn change_password(State(app): S, h: HeaderMap, Json(r): Json<PasswordReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || {
        let st = krypton_wallet_core::password::strength(&r.new);
        ensure!(st.ok, "{}", st.hint);
        ensure!(
            r.new != r.old,
            "the new password is the same as the old one"
        );
        let path = a.vault.path(&a.dir, &r.id).context("no such wallet")?;
        // The old password must open the file as it is on disk.
        krypton_wallet_core::file::FileKey::open(&path, &r.old)
            .map_err(|_| anyhow::anyhow!("the current password is wrong"))?;
        // Change the open copy if there is one: it would otherwise save under the old key.
        {
            let mut g = a.wallet.lock();
            if let Some(w) = g.as_mut()
                && w.path() == path.as_path()
            {
                return w.change_password(&r.new, Argon2Params::STRONG);
            }
        }
        if let Some(w) = a.vault.others.lock().get_mut(&r.id)
            && w.path() == path.as_path()
        {
            return w.change_password(&r.new, Argon2Params::STRONG);
        }
        anyhow::ensure!(
            !a.vault.syncing.lock().contains(&r.id),
            "the wallet is busy syncing; try again in a few seconds"
        );
        let mut w = Wallet::open(&path, &r.old)?;
        w.change_password(&r.new, Argon2Params::STRONG)
    })
    .await
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct StrengthReq {
    password: String,
}

impl Drop for StrengthReq {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}

/// Strength of a candidate password (for the meter while typing; nothing is stored).
pub async fn password_strength(
    State(app): S,
    h: HeaderMap,
    Json(r): Json<StrengthReq>,
) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    Json(krypton_wallet_core::password::strength(&r.password)).into_response()
}

pub async fn lock_one(State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    let _ = tokio::task::spawn_blocking(move || lock_wallet(&a, &r.id)).await;
    Json(json!({"ok": true})).into_response()
}

pub async fn lock_all(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let ids: Vec<String> = a.vault.unlocked.lock().keys().cloned().collect();
        for id in ids {
            lock_wallet(&a, &id);
        }
    })
    .await;
    Json(json!({"ok": true})).into_response()
}

pub async fn use_wallet(State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    match activate(&app, &r.id) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn rename(State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let res = (|| -> Result<()> {
        let name = valid_name(r.name.as_deref().unwrap_or(""))?;
        let mut reg = app.vault.reg.lock();
        let e = reg
            .wallets
            .iter_mut()
            .find(|e| e.id == r.id)
            .context("no such wallet")?;
        e.name = name;
        reg.save(&app.dir)
    })();
    match res {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

/// Removes a wallet from the app. Its file is moved to `removed/`, never destroyed:
/// the coins stay safe and the 24 words (or that file) bring it back.
pub async fn remove(State(app): S, h: HeaderMap, Json(r): Json<IdReq>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let res = (|| -> Result<()> {
        let entry = app
            .vault
            .reg
            .lock()
            .get(&r.id)
            .cloned()
            .context("no such wallet")?;
        lock_wallet(&app, &r.id);
        app.vault.cache.lock().remove(&r.id);
        let src = app.dir.join(&entry.file);
        if src.exists() {
            let bin = app.dir.join("removed");
            std::fs::create_dir_all(&bin)?;
            let dst = bin.join(format!(
                "{}-{}-{}.krypton",
                entry.id,
                krypton_core::now(),
                sanitize(&entry.name)
            ));
            std::fs::rename(&src, &dst)?;
        }
        let mut reg = app.vault.reg.lock();
        reg.wallets.retain(|e| e.id != r.id);
        if reg.active == r.id {
            reg.active = reg
                .wallets
                .first()
                .map(|e| e.id.clone())
                .unwrap_or_default();
            let next = reg.active.clone();
            drop(reg);
            let w = app.vault.others.lock().remove(&next);
            *app.wallet.lock() = w;
            app.vault.reg.lock().save(&app.dir)?;
        } else {
            reg.save(&app.dir)?;
        }
        Ok(())
    })();
    match res {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

fn sanitize(n: &str) -> String {
    n.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(24)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_migrates_and_roundtrips() {
        let d = tempdir();
        assert!(Registry::load(&d).wallets.is_empty());
        std::fs::write(d.join("wallet.krypton"), b"x").unwrap();
        let r = Registry::load(&d);
        assert_eq!(r.active, "main");
        assert_eq!(r.wallets[0].file, "wallet.krypton");
        let mut r2 = r.clone();
        r2.wallets.push(Entry {
            id: "abcd".into(),
            name: "Savings".into(),
            file: "wallets/abcd.krypton".into(),
            created: 1,
            fp: String::new(),
            addr: String::new(),
        });
        r2.save(&d).unwrap();
        let r3 = Registry::load(&d);
        assert_eq!(r3.wallets.len(), 2);
        assert_eq!(r3.get("abcd").unwrap().name, "Savings");
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn names_are_checked() {
        assert!(valid_name("  ").is_err());
        assert!(valid_name(&"x".repeat(41)).is_err());
        assert_eq!(valid_name("  Mining rig 2 ").unwrap(), "Mining rig 2");
    }
    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gk-vault-{}",
            hex::encode(rand::random::<[u8; 6]>())
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
