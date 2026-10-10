//! Many unlocked wallets in one window (up to 10,000). The active wallet lives in
//! the app's own fields; the others wait here with their last scan. Remembered
//! wallets unlock and scan in batches through one CLI call each, so a large set
//! never means thousands of processes. Lock closes all of them.
use super::*;
use std::collections::{HashMap, HashSet};

pub(super) const MAX_OPEN_WALLETS: usize = 10_000;
/// Wallets per batch call: key derivation for this many takes a few seconds,
/// so results arrive steadily and a lock never waits long.
pub(super) const BATCH_SIZE: usize = 250;
/// Wallets listed by name in the top-bar menu; the Wallets page lists all.
const SWITCHER_ROWS: usize = 12;
const BACKGROUND_SYNC: &str = "sync@";
pub(super) const BATCH_UNLOCK: &str = "batch:unlock";
pub(super) const BATCH_SCAN: &str = "batch:scan";

pub(super) struct WalletSession {
    pub path: String,
    pub password: Zeroizing<String>,
    pub address: String,
    pub balance: Option<String>,
    pub spendable: Option<String>,
    pub height: Option<u64>,
    pub history: Vec<Value>,
    pub detail: Option<Value>,
    pub last_sync: Option<Instant>,
}
impl Drop for WalletSession {
    fn drop(&mut self) {
        self.address.zeroize();
        for value in &mut self.history {
            protocol::wipe_json(value);
        }
        if let Some(detail) = &mut self.detail {
            protocol::wipe_json(detail);
        }
    }
}
impl WalletSession {
    /// A wallet opened by a batch, with the summary the batch returned.
    fn from_batch(path: String, password: Zeroizing<String>, result: &Value) -> Self {
        let mut session = Self {
            path,
            password,
            address: String::new(),
            balance: None,
            spendable: None,
            height: None,
            history: Vec::new(),
            detail: None,
            last_sync: None,
        };
        session.apply_summary(result);
        session
    }
    fn apply_summary(&mut self, result: &Value) {
        if let Some(address) = result["address"].as_str() {
            self.address = address.into();
        }
        if let Some(balance) = result["balance_atomic"].as_str() {
            self.balance = Some(balance.to_owned());
            self.spendable = result["spendable_atomic"].as_str().map(str::to_owned);
            self.height = result["height"].as_u64();
            if let Some(detail) = &mut self.detail {
                protocol::wipe_json(detail);
            }
            self.detail = Some(controller::sync_detail(result));
            self.last_sync = Some(Instant::now());
        }
    }
}

/// The file name without its extension, as shown in the wallet lists.
pub(super) fn wallet_name(path: &str) -> String {
    PathBuf::from(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Wallet".into())
}
/// A path as the filesystem compares it: separators unified, and case-folded on
/// Windows.
pub(super) fn path_key(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    }
}
pub(super) fn same_path(a: &str, b: &str) -> bool {
    path_key(a) == path_key(b)
}
pub(super) fn balance_value(balance: Option<&str>) -> Option<u128> {
    balance.and_then(|value| value.parse::<u128>().ok())
}
pub(super) fn sum_balances<'a>(balances: impl Iterator<Item = Option<&'a str>>) -> (u128, usize) {
    let mut total = 0u128;
    let mut unknown = 0;
    for balance in balances {
        match balance_value(balance) {
            Some(value) => total = total.saturating_add(value),
            None => unknown += 1,
        }
    }
    (total, unknown)
}
/// Richest first; wallets not scanned yet go last; equal balances by name.
pub(super) fn richest_first(
    a: (Option<u128>, &str),
    b: (Option<u128>, &str),
) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| {
        wallet_name(a.1)
            .to_lowercase()
            .cmp(&wallet_name(b.1).to_lowercase())
    })
}
fn amount_text(atomic: u128) -> String {
    format!("{} GNR", format_atomic(&atomic.to_string()))
}

impl WalletApp {
    fn invalidate_active_requests(&mut self) {
        self.cancellation.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.auto_scan_running = false;
    }
    fn clear_drafts(&mut self) {
        self.forget_name_identity();
        self.preview = None;
        self.proofs = payment_proof::ProofUi::default();
        self.to.zeroize();
        self.amount.zeroize();
        self.memo.zeroize();
        self.request_amount.zeroize();
        self.payment_request = None;
        self.last_submission = None;
    }
    /// Moves the active wallet out of the app fields, leaving the gate visible.
    fn take_active(&mut self) -> Option<WalletSession> {
        if !self.unlocked {
            return None;
        }
        self.invalidate_active_requests();
        self.clear_drafts();
        self.unlocked = false;
        Some(WalletSession {
            path: self.settings.wallet_path.clone(),
            password: std::mem::take(&mut self.password),
            address: std::mem::take(&mut self.address),
            balance: self.balance.take(),
            spendable: self.spendable.take(),
            height: self.wallet_height.take(),
            history: std::mem::take(&mut self.history),
            detail: self.sync_detail.take(),
            last_sync: self.last_sync.take(),
        })
    }
    fn activate(&mut self, mut session: WalletSession) {
        self.invalidate_active_requests();
        self.clear_drafts();
        self.settings.wallet_path = std::mem::take(&mut session.path);
        self.password = std::mem::take(&mut session.password);
        self.address = std::mem::take(&mut session.address);
        self.balance = session.balance.take();
        self.spendable = session.spendable.take();
        self.wallet_height = session.height.take();
        self.history = std::mem::take(&mut session.history);
        self.sync_detail = session.detail.take();
        self.last_sync = session.last_sync.take();
        self.unlocked = true;
        self.adding_wallet = false;
    }
    pub(super) fn open_wallet_count(&self) -> usize {
        self.sessions.len() + usize::from(self.unlocked)
    }
    /// Keys of every open wallet's path, for set lookups over large lists.
    pub(super) fn open_paths(&self) -> HashSet<String> {
        let mut open: HashSet<String> = self
            .sessions
            .iter()
            .map(|session| path_key(&session.path))
            .collect();
        if self.unlocked {
            open.insert(path_key(&self.settings.wallet_path));
        }
        open
    }
    pub(super) fn is_wallet_open(&self, path: &str) -> bool {
        let key = path_key(path);
        self.sessions
            .iter()
            .any(|session| path_key(&session.path) == key)
            || (self.unlocked && path_key(&self.settings.wallet_path) == key)
    }
    /// Total of every open wallet's last scan, and how many are not scanned yet.
    pub(super) fn combined_balance(&self) -> (u128, usize) {
        let active = self.unlocked.then_some(self.balance.as_deref());
        sum_balances(
            active.into_iter().chain(
                self.sessions
                    .iter()
                    .map(|session| session.balance.as_deref()),
            ),
        )
    }
    /// Indices of the `count` richest background wallets, richest first. Only
    /// those are fully sorted, so a short list from 10,000 stays cheap per frame.
    pub(super) fn top_sessions(&self, count: usize) -> Vec<usize> {
        if count == 0 {
            return Vec::new();
        }
        let mut order: Vec<(Option<u128>, usize)> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (balance_value(session.balance.as_deref()), index))
            .collect();
        let by = |a: &(Option<u128>, usize), b: &(Option<u128>, usize)| {
            richest_first(
                (a.0, &self.sessions[a.1].path),
                (b.0, &self.sessions[b.1].path),
            )
        };
        if order.len() > count {
            order.select_nth_unstable_by(count - 1, by);
            order.truncate(count);
        }
        order.sort_by(by);
        order.into_iter().map(|(_, index)| index).collect()
    }
    pub(super) fn switch_wallet(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        let target = self.sessions.remove(index);
        if let Some(current) = self.take_active() {
            self.sessions.insert(index, current);
        }
        self.activate(target);
        self.page = self.page.min(SECTIONS.len() - 1);
        self.last_auto_scan = Instant::now() - self.auto_scan_interval();
        self.status = format!(
            "Switched to wallet {}.",
            wallet_name(&self.settings.wallet_path)
        );
    }
    pub(super) fn begin_add_wallet(&mut self) {
        if self.open_wallet_count() >= MAX_OPEN_WALLETS {
            self.status = format!(
                "At most {} wallets can be open at once.",
                grouped_count(MAX_OPEN_WALLETS)
            );
            return;
        }
        if let Some(current) = self.take_active() {
            self.sessions.push(current);
        }
        self.adding_wallet = true;
        self.password.zeroize();
        self.confirm_password.zeroize();
        self.seed_input.zeroize();
        self.page = 0;
        self.wallet_mode = 0;
        self.prepare_wallet_form(0);
        if self.is_wallet_open(&self.settings.wallet_path) {
            self.wallet_mode = 1;
            self.prepare_wallet_form(1);
        }
    }
    pub(super) fn cancel_add_wallet(&mut self) {
        self.adding_wallet = false;
        self.password.zeroize();
        self.confirm_password.zeroize();
        self.seed_input.zeroize();
        if let Some(last) = self.sessions.pop() {
            self.activate(last);
        }
    }
    /// Closes only the active wallet and stops remembering it; another open wallet
    /// becomes active.
    pub(super) fn close_active_wallet(&mut self) {
        let path = self.settings.wallet_path.clone();
        if let Some(mut closed) = self.take_active() {
            closed.password.zeroize();
        }
        self.preferences.forget(&path);
        self.save_preferences();
        if self.sessions.is_empty() {
            self.lock();
            return;
        }
        let next = self.sessions.remove(0);
        self.activate(next);
        self.status = format!(
            "Wallet closed. {} is now active.",
            wallet_name(&self.settings.wallet_path)
        );
    }
    /// Locks one background wallet; it stays remembered.
    pub(super) fn close_session(&mut self, index: usize) {
        if index < self.sessions.len() {
            let mut closed = self.sessions.remove(index);
            closed.password.zeroize();
            self.status = format!("{} locked.", wallet_name(&closed.path));
        }
    }
    /// Stops remembering a wallet that is not open; its file stays untouched.
    pub(super) fn forget_wallet(&mut self, path: &str) {
        if self.is_wallet_open(path) {
            return;
        }
        self.preferences.forget(path);
        self.unlock_failures.remove(&path_key(path));
        self.save_preferences();
        self.status = format!(
            "{} removed from the list. The wallet file was not touched.",
            wallet_name(path)
        );
    }
    /// Scans an inactive wallet now; its result updates only that wallet.
    pub(super) fn scan_session(&mut self, index: usize) {
        let Some(session) = self.sessions.get(index) else {
            return;
        };
        let label = format!("{BACKGROUND_SYNC}{}", session.path);
        let credentials = Some((session.path.clone(), session.password.clone()));
        self.dispatch("sync", label, credentials, json!({}));
    }
    /// Applies a background scan result; returns false when the label is not one.
    pub(super) fn apply_background_sync(&mut self, label: &str, result: &Value) -> bool {
        let Some(path) = label.strip_prefix(BACKGROUND_SYNC) else {
            return false;
        };
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| same_path(&session.path, path))
        {
            session.balance = result["balance_atomic"].as_str().map(str::to_owned);
            session.spendable = result["spendable_atomic"].as_str().map(str::to_owned);
            session.height = result["height"].as_u64();
            for value in &mut session.history {
                protocol::wipe_json(value);
            }
            session.history = result["history"].as_array().cloned().unwrap_or_default();
            if let Some(detail) = &mut session.detail {
                protocol::wipe_json(detail);
            }
            session.detail = Some(controller::sync_detail(result));
            session.last_sync = Some(Instant::now());
            let path = session.path.clone();
            self.notifier.observe(&path, result);
            if let Some(address) = result["address"].as_str() {
                session.address = address.into();
            }
        }
        true
    }
    pub(super) fn save_preferences(&mut self) {
        if let Err(error) = preferences::save(&self.settings.data_dir, &self.preferences) {
            self.status = format!("Could not save wallet preferences: {error}");
        }
    }
    /// Remembered wallets that are not open now, in remembered order. A file
    /// deleted meanwhile is reported when it is unlocked.
    pub(super) fn remembered_locked(&self) -> Vec<String> {
        let open = self.open_paths();
        self.preferences
            .wallets
            .iter()
            .filter(|path| !open.contains(&path_key(path)))
            .cloned()
            .collect()
    }
    /// After the active wallet unlocks: remember it and, if chosen, try the same
    /// password on the other remembered wallets.
    pub(super) fn after_unlock(&mut self, try_others: bool) {
        self.preferences.remember(&self.settings.wallet_path);
        self.unlock_failures
            .remove(&path_key(&self.settings.wallet_path));
        self.save_preferences();
        if try_others && self.unlock_others {
            let password = self.password.clone();
            self.queue_unlocks(self.remembered_locked(), password);
        }
    }
    /// Queues wallets to unlock in the background with one password.
    pub(super) fn queue_unlocks(&mut self, paths: Vec<String>, password: Zeroizing<String>) {
        let queued: HashSet<String> = self.pending_unlocks.iter().map(|p| path_key(p)).collect();
        let fresh: Vec<String> = paths
            .into_iter()
            .filter(|path| !queued.contains(&path_key(path)))
            .collect();
        if fresh.is_empty() {
            return;
        }
        let (done, total) = self.unlock_progress.unwrap_or((0, 0));
        self.unlock_progress = Some((done, total + fresh.len()));
        self.pending_unlocks.extend(fresh);
        self.pending_password = Some(password);
    }
    /// Sends the next batch of queued wallets once the scan lane is free.
    pub(super) fn process_pending_unlocks(&mut self) {
        if self.scan_busy.is_some() || !self.unlocked || !self.unlock_inflight.is_empty() {
            return;
        }
        let Some(password) = self.pending_password.clone() else {
            self.pending_unlocks.clear();
            self.unlock_progress = None;
            return;
        };
        let open = self.open_paths();
        let room = MAX_OPEN_WALLETS.saturating_sub(self.open_wallet_count());
        let mut chunk = Vec::new();
        while chunk.len() < BATCH_SIZE.min(room) {
            let Some(path) = self.pending_unlocks.pop_front() else {
                break;
            };
            if open.contains(&path_key(&path)) {
                self.count_unlock_done(1);
            } else {
                chunk.push(path);
            }
        }
        if chunk.is_empty() {
            self.pending_unlocks.clear();
            self.pending_password = None;
            self.unlock_progress = None;
            return;
        }
        let wallets: Vec<Value> = chunk
            .iter()
            .map(|path| json!({"wallet_path": path, "password": password.as_str()}))
            .collect();
        self.unlock_inflight = chunk;
        let credentials = Some((self.settings.wallet_path.clone(), self.password.clone()));
        self.dispatch(
            "sync_batch",
            BATCH_UNLOCK.into(),
            credentials,
            json!({"wallets": wallets}),
        );
        if self.scan_busy.is_none() {
            // The CLI could not start: the queue ends, nothing is half open.
            self.unlock_inflight.clear();
            self.pending_unlocks.clear();
            self.pending_password = None;
            self.unlock_progress = None;
        }
    }
    fn count_unlock_done(&mut self, count: usize) {
        if let Some((done, total)) = &mut self.unlock_progress {
            *done = (*done + count).min(*total);
        }
    }
    /// Scans up to one batch of background wallets that are behind `node_height`,
    /// furthest behind first.
    pub(super) fn scan_sessions_batch(&mut self, node_height: u64) -> bool {
        let mut behind: Vec<usize> = (0..self.sessions.len())
            .filter(|&index| {
                self.sessions[index]
                    .height
                    .is_none_or(|height| height < node_height)
            })
            .collect();
        if behind.is_empty() {
            return false;
        }
        behind.sort_by_key(|&index| self.sessions[index].height.unwrap_or(0));
        let wallets: Vec<Value> = behind
            .iter()
            .take(BATCH_SIZE)
            .map(|&index| {
                let session = &self.sessions[index];
                json!({"wallet_path": session.path, "password": session.password.as_str()})
            })
            .collect();
        let credentials = Some((self.settings.wallet_path.clone(), self.password.clone()));
        self.dispatch(
            "sync_batch",
            BATCH_SCAN.into(),
            credentials,
            json!({"wallets": wallets}),
        );
        self.scan_busy.is_some()
    }
    /// Applies a batch result; returns false when the label is not a batch.
    pub(super) fn apply_batch(&mut self, label: &str, result: &Value) -> bool {
        let unlocking = label == BATCH_UNLOCK;
        if !unlocking && label != BATCH_SCAN {
            return false;
        }
        let inflight: HashMap<String, String> = std::mem::take(&mut self.unlock_inflight)
            .into_iter()
            .map(|path| (path_key(&path), path))
            .collect();
        let password = self.pending_password.clone();
        let mut index: HashMap<String, usize> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, session)| (path_key(&session.path), i))
            .collect();
        let active = self.unlocked.then(|| path_key(&self.settings.wallet_path));
        let (mut opened, mut failed, mut gained, mut gained_wallets) = (0, 0, 0u128, Vec::new());
        for entry in result["wallets"].as_array().into_iter().flatten() {
            let path = entry["wallet_path"].as_str().unwrap_or_default();
            let key = path_key(path);
            if active.as_ref() == Some(&key) {
                continue;
            }
            if entry["ok"] != true {
                if unlocking && inflight.contains_key(&key) {
                    failed += 1;
                    let error = entry["error"].as_str().unwrap_or("could not be opened");
                    self.unlock_failures.insert(key, unlock_failure_text(error));
                }
                continue;
            }
            match index.get(&key) {
                Some(&i) => {
                    let session = &mut self.sessions[i];
                    let before = balance_value(session.balance.as_deref());
                    session.apply_summary(entry);
                    let after = balance_value(session.balance.as_deref());
                    if let (Some(before), Some(after)) = (before, after)
                        && after > before
                    {
                        gained += after - before;
                        gained_wallets.push(session.path.clone());
                    }
                }
                None if unlocking => {
                    let (Some(original), Some(password)) = (inflight.get(&key), &password) else {
                        continue;
                    };
                    if self.sessions.len() + usize::from(self.unlocked) >= MAX_OPEN_WALLETS {
                        continue;
                    }
                    self.unlock_failures.remove(&key);
                    self.sessions.push(WalletSession::from_batch(
                        original.clone(),
                        password.clone(),
                        entry,
                    ));
                    index.insert(key, self.sessions.len() - 1);
                    opened += 1;
                }
                None => {}
            }
        }
        if unlocking {
            self.count_unlock_done(inflight.len());
            let remaining = self.pending_unlocks.len();
            self.status = match (opened, failed, remaining) {
                (_, _, 1..) => format!(
                    "Unlocking wallets: {} of {} done…",
                    grouped_count(self.unlock_progress.map_or(0, |p| p.0)),
                    grouped_count(self.unlock_progress.map_or(0, |p| p.1)),
                ),
                (_, 0, 0) => format!(
                    "{} wallets are open. Every wallet is on the Wallets page.",
                    grouped_count(self.open_wallet_count())
                ),
                (_, failed, 0) => format!(
                    "{} wallets are open; {} could not be opened with this password — see the Wallets page.",
                    grouped_count(self.open_wallet_count()),
                    grouped_count(failed)
                ),
            };
            if remaining == 0 {
                self.pending_password = None;
                self.unlock_progress = None;
            }
        }
        match gained_wallets.as_slice() {
            [] => {}
            [only] => self.notifier.announce(format!(
                "{}: balance up {}",
                wallet_name(only),
                amount_text(gained)
            )),
            many => self.notifier.announce(format!(
                "{} other wallets received funds: +{}",
                grouped_count(many.len()),
                amount_text(gained)
            )),
        }
        true
    }
    /// A batch call that failed as a whole (the CLI crashed or timed out).
    pub(super) fn batch_failed(&mut self, label: &str, error: &str) -> bool {
        match label {
            BATCH_UNLOCK => {
                let chunk = std::mem::take(&mut self.unlock_inflight);
                self.count_unlock_done(chunk.len());
                for path in chunk {
                    self.unlock_failures
                        .insert(path_key(&path), unlock_failure_text(error));
                }
                self.status = format!("Some wallets could not be unlocked: {error}");
                true
            }
            BATCH_SCAN => {
                self.status = format!("Background scan of other wallets failed: {error}");
                true
            }
            _ => false,
        }
    }
    /// A batch unlock whose result was discarded is tried again.
    pub(super) fn requeue_stale_unlock(&mut self, label: &str) {
        if label == BATCH_UNLOCK && self.pending_password.is_some() {
            for path in std::mem::take(&mut self.unlock_inflight).into_iter().rev() {
                self.pending_unlocks.push_front(path);
            }
        } else if label == BATCH_UNLOCK {
            self.unlock_inflight.clear();
        }
    }
    pub(super) fn begin_unlock_remembered(&mut self, path: &str) {
        if self.unlocked {
            self.begin_add_wallet();
            if !self.adding_wallet {
                return;
            }
        }
        self.page = 0;
        self.wallet_mode = 0;
        self.settings.wallet_path = path.to_owned();
        self.password.zeroize();
    }
    /// The top-bar wallet menu: the richest open wallets, a way to every wallet,
    /// adding another, or closing the active one.
    pub(super) fn wallet_switcher(&mut self, ui: &mut egui::Ui) {
        if !self.unlocked && self.sessions.is_empty() {
            ui.label(
                RichText::new("GENORYN / WALLET")
                    .monospace()
                    .small()
                    .color(GRAY),
            );
            return;
        }
        let title = if self.unlocked {
            wallet_name(&self.settings.wallet_path)
        } else {
            "Adding a wallet".to_owned()
        };
        enum Action {
            Switch(usize),
            AllWallets,
            Add,
            Close,
        }
        let mut action = None;
        let enabled = self.busy.is_none() && self.seed_backup.is_empty() && !self.process_busy;
        ui.add_enabled_ui(enabled, |ui| {
            egui::ComboBox::from_id_salt("wallet_switcher")
                .selected_text(RichText::new(title).strong())
                .width(240.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(320.0);
                    if self.unlocked {
                        ui.label(RichText::new("ACTIVE").small().color(GRAY));
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(wallet_name(&self.settings.wallet_path)).strong(),
                            );
                            gnr_amount(ui, self.balance.as_deref(), 13.0, None);
                        });
                        ui.separator();
                    }
                    let order = self.top_sessions(SWITCHER_ROWS);
                    if !order.is_empty() {
                        ui.label(RichText::new("TOP WALLETS BY BALANCE").small().color(GRAY));
                    }
                    for &index in &order {
                        let session = &self.sessions[index];
                        let label = format!(
                            "{}   {} GNR",
                            wallet_name(&session.path),
                            session
                                .balance
                                .as_deref()
                                .map(format_atomic)
                                .unwrap_or_else(|| "—".into())
                        );
                        if ui
                            .selectable_label(false, label)
                            .on_hover_text(&session.path)
                            .clicked()
                        {
                            action = Some(Action::Switch(index));
                        }
                    }
                    ui.separator();
                    let total = self.open_wallet_count() + self.remembered_locked_count();
                    if ui
                        .add(
                            egui::Button::new(format!("All wallets ({})…", grouped_count(total)))
                                .frame(false),
                        )
                        .clicked()
                    {
                        action = Some(Action::AllWallets);
                    }
                    if self.unlocked
                        && ui
                            .add_enabled(
                                self.open_wallet_count() < MAX_OPEN_WALLETS,
                                egui::Button::new("Open another wallet…").frame(false),
                            )
                            .clicked()
                    {
                        action = Some(Action::Add);
                    }
                    if self.unlocked
                        && ui
                            .add(egui::Button::new("Close this wallet").frame(false))
                            .clicked()
                    {
                        action = Some(Action::Close);
                    }
                });
        });
        match action {
            Some(Action::Switch(index)) => {
                if self.adding_wallet {
                    self.adding_wallet = false;
                    let target = self.sessions.remove(index);
                    self.activate(target);
                } else {
                    self.switch_wallet(index);
                }
            }
            Some(Action::AllWallets) => {
                if self.adding_wallet {
                    self.cancel_add_wallet();
                }
                self.page = WALLETS_PAGE;
            }
            Some(Action::Add) => self.begin_add_wallet(),
            Some(Action::Close) => self.close_active_wallet(),
            None => {}
        }
    }
    /// Remembered wallets that are not open, counted without allocating a list.
    pub(super) fn remembered_locked_count(&self) -> usize {
        let open = self.open_paths();
        self.preferences
            .wallets
            .iter()
            .filter(|path| !open.contains(&path_key(path)))
            .count()
    }
}

/// What the Wallets page shows for a wallet that did not open.
fn unlock_failure_text(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("decrypt") || lower.contains("password") || lower.contains("authentic") {
        "Different password".into()
    } else if lower.contains("cannot find")
        || lower.contains("no such file")
        || lower.contains("not found")
        || lower.contains("os error 2")
        || lower.contains("os error 3")
    {
        "File missing".into()
    } else if lower.contains("network mismatch") {
        "Another network".into()
    } else {
        error.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(path: &str, balance: Option<&str>) -> WalletSession {
        WalletSession {
            path: path.into(),
            password: Zeroizing::new("correct-horse".into()),
            address: format!("gnrd-{path}"),
            balance: balance.map(str::to_owned),
            spendable: balance.map(str::to_owned),
            height: Some(3),
            history: Vec::new(),
            detail: None,
            last_sync: None,
        }
    }
    fn unlocked_app(ctx: &egui::Context, path: &str, balance: &str) -> WalletApp {
        let mut app = WalletApp::with_context(ctx, Settings::default());
        app.settings.data_dir = std::env::temp_dir()
            .join("genoryn-sessions-test-absent")
            .display()
            .to_string();
        app.settings.wallet_path = path.into();
        app.unlocked = true;
        *app.password = "active-password".into();
        app.address = "gnrd-active".into();
        app.balance = Some(balance.into());
        app
    }

    #[test]
    fn names_and_paths_compare_like_the_filesystem() {
        assert_eq!(wallet_name("C:/wallets/savings.gnr"), "savings");
        assert_eq!(wallet_name(""), "Wallet");
        assert!(same_path("a/b/wallet.gnr", "a\\b\\wallet.gnr"));
        assert_eq!(cfg!(windows), same_path("A/Wallet.GNR", "a/wallet.gnr"));
    }

    #[test]
    fn totals_add_exactly_and_count_unscanned_wallets() {
        let (total, unknown) = sum_balances(
            [
                Some("1000000000000"),
                None,
                Some("250000000000"),
                Some("bad"),
            ]
            .into_iter(),
        );
        assert_eq!(total, 1_250_000_000_000);
        assert_eq!(unknown, 2);
    }

    #[test]
    fn richest_wallets_come_first_and_unscanned_ones_last() {
        let ctx = egui::Context::default();
        let mut app = unlocked_app(&ctx, "active.gnr", "1");
        app.sessions.push(session("small.gnr", Some("5")));
        app.sessions.push(session("unknown.gnr", None));
        app.sessions
            .push(session("big.gnr", Some("900000000000000000000")));
        app.sessions.push(session("Also-small.gnr", Some("5")));
        let names = |count: usize| -> Vec<String> {
            app.top_sessions(count)
                .into_iter()
                .map(|index| wallet_name(&app.sessions[index].path))
                .collect()
        };
        assert_eq!(names(10), ["big", "Also-small", "small", "unknown"]);
        assert_eq!(names(2), ["big", "Also-small"]);
        assert!(names(0).is_empty());
    }

    #[test]
    fn switching_swaps_wallets_without_mixing_their_state() {
        let ctx = egui::Context::default();
        let mut app = unlocked_app(&ctx, "one.gnr", "5");
        app.sessions.push(session("two.gnr", Some("7")));
        app.to = "draft recipient".into();
        let generation = app.generation;
        assert_eq!(app.combined_balance(), (12, 0));
        app.switch_wallet(0);
        assert_eq!(app.settings.wallet_path, "two.gnr");
        assert_eq!(app.password.as_str(), "correct-horse");
        assert_eq!(app.balance.as_deref(), Some("7"));
        assert!(
            app.to.is_empty(),
            "drafts never carry over to another wallet"
        );
        assert_ne!(
            app.generation, generation,
            "late results of the old wallet are ignored"
        );
        assert_eq!(app.sessions[0].path, "one.gnr");
        assert_eq!(app.sessions[0].password.as_str(), "active-password");
        assert_eq!(app.combined_balance(), (12, 0));
        assert!(app.is_wallet_open("one.gnr") && app.is_wallet_open("two.gnr"));
    }

    #[test]
    fn adding_closing_and_locking_wallets() {
        let ctx = egui::Context::default();
        let mut app = unlocked_app(&ctx, "one.gnr", "5");
        app.begin_add_wallet();
        assert!(app.adding_wallet && !app.unlocked);
        assert!(app.password.is_empty());
        assert_eq!(app.sessions.len(), 1);
        app.cancel_add_wallet();
        assert!(app.unlocked && !app.adding_wallet);
        assert_eq!(app.settings.wallet_path, "one.gnr");
        app.sessions.push(session("two.gnr", None));
        app.close_active_wallet();
        assert_eq!(app.settings.wallet_path, "two.gnr");
        assert!(app.sessions.is_empty() && app.unlocked);
        app.sessions.push(session("three.gnr", Some("1")));
        app.close_session(0);
        assert!(app.sessions.is_empty() && app.unlocked);
        app.sessions.push(session("four.gnr", Some("1")));
        app.lock();
        assert!(
            app.sessions.is_empty() && !app.unlocked,
            "lock closes every wallet"
        );
    }

    #[test]
    fn remembered_wallets_unlock_in_batches_and_failures_wait_on_the_wallets_page() {
        let ctx = egui::Context::default();
        let folder = std::env::temp_dir().join(format!(
            "genoryn-remembered-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let file = |name: &str| folder.join(name).display().to_string();
        let (one, two, three) = (file("one.gnr"), file("two.gnr"), file("three.gnr"));
        let mut app = unlocked_app(&ctx, &one, "5");
        app.settings.data_dir = folder.display().to_string();
        app.settings.bin_dir = folder.join("no-binaries").display().to_string();
        app.preferences.remember(&three);
        app.preferences.remember(&two);
        app.after_unlock(true);
        assert_eq!(
            app.preferences.wallets[0], one,
            "the unlocked wallet is remembered first"
        );
        assert_eq!(preferences::load(&app.settings.data_dir).wallets[0], one);
        assert_eq!(app.pending_unlocks, [two.clone(), three.clone()]);
        assert_eq!(app.unlock_progress, Some((0, 2)));
        assert_eq!(
            app.pending_password.as_deref().map(String::as_str),
            Some("active-password")
        );

        app.unlock_inflight = vec![two.clone(), three.clone()];
        app.pending_unlocks.clear();
        let result = json!({"wallets": [
            {"wallet_path": two, "ok": true, "address": "gnrd-two", "balance_atomic": "7",
             "spendable_atomic": "7", "height": 9},
            {"wallet_path": three, "ok": false, "error": "wallet decryption failed"},
        ]});
        assert!(app.apply_batch(BATCH_UNLOCK, &result));
        assert!(app.is_wallet_open(&two));
        assert_eq!(app.sessions[0].password.as_str(), "active-password");
        assert_eq!(app.sessions[0].address, "gnrd-two");
        assert_eq!(app.sessions[0].balance.as_deref(), Some("7"));
        assert_eq!(app.sessions[0].height, Some(9));
        assert!(!app.is_wallet_open(&three));
        assert_eq!(
            app.unlock_failures
                .get(&path_key(&three))
                .map(String::as_str),
            Some("Different password")
        );
        assert_eq!(app.remembered_locked(), std::slice::from_ref(&three));
        assert!(app.pending_password.is_none() && app.unlock_progress.is_none());

        app.pending_password = Some(Zeroizing::new("x".into()));
        app.unlock_inflight = vec![three.clone()];
        app.requeue_stale_unlock(BATCH_UNLOCK);
        assert_eq!(
            app.pending_unlocks[0], three,
            "a discarded batch is retried"
        );
        assert!(app.unlock_inflight.is_empty());

        app.process_pending_unlocks();
        assert!(app.unlock_inflight.is_empty() && app.pending_unlocks.is_empty());
        assert!(
            app.pending_password.is_none(),
            "a CLI that cannot start ends the queue"
        );

        app.forget_wallet(&three);
        assert!(!app.preferences.wallets.contains(&three));
        app.close_active_wallet();
        assert!(
            !app.preferences.wallets.contains(&one),
            "closing forgets the wallet"
        );
        app.lock();
        assert!(app.pending_password.is_none() && app.sessions.is_empty());
        assert!(app.unlock_failures.is_empty());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn ten_thousand_wallets_open_from_batches_without_duplicates() {
        let ctx = egui::Context::default();
        let mut app = unlocked_app(&ctx, "active.gnr", "1");
        let paths: Vec<String> = (0..MAX_OPEN_WALLETS + 5)
            .map(|n| format!("w/{n}.gnr"))
            .collect();
        app.queue_unlocks(paths.clone(), Zeroizing::new("pw".into()));
        app.queue_unlocks(paths.clone(), Zeroizing::new("pw".into()));
        assert_eq!(app.pending_unlocks.len(), paths.len(), "queued once");
        assert_eq!(app.unlock_progress, Some((0, paths.len())));
        for chunk in paths.chunks(BATCH_SIZE) {
            let take = chunk.len().min(app.pending_unlocks.len());
            app.unlock_inflight = app.pending_unlocks.drain(..take).collect();
            let wallets: Vec<Value> = chunk
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    json!({"wallet_path": path, "ok": true, "address": "a",
                           "balance_atomic": (i as u64 * 3).to_string(), "height": 1})
                })
                .collect();
            assert!(app.apply_batch(BATCH_UNLOCK, &json!({ "wallets": wallets })));
        }
        assert_eq!(app.open_wallet_count(), MAX_OPEN_WALLETS, "the limit holds");
        assert_eq!(app.open_paths().len(), MAX_OPEN_WALLETS);
        assert!(
            app.pending_password.is_none() && app.unlock_progress.is_none(),
            "the password is dropped once the queue is done"
        );
        let top = app.top_sessions(1)[0];
        assert_eq!(
            app.sessions[top].balance.as_deref(),
            Some(((BATCH_SIZE as u64 - 1) * 3).to_string().as_str())
        );
    }

    #[test]
    fn background_results_update_only_their_own_wallet() {
        let ctx = egui::Context::default();
        let mut app = unlocked_app(&ctx, "one.gnr", "5");
        app.sessions.push(session("two.gnr", None));
        let result =
            json!({"balance_atomic": "9", "spendable_atomic": "8", "height": 10, "history": []});
        assert!(app.apply_background_sync("sync@two.gnr", &result));
        assert!(!app.apply_background_sync("sync", &result));
        assert_eq!(app.sessions[0].balance.as_deref(), Some("9"));
        assert_eq!(app.balance.as_deref(), Some("5"));

        let batch = json!({"wallets": [
            {"wallet_path": "two.gnr", "ok": true, "balance_atomic": "1000000000009", "height": 11},
            {"wallet_path": "one.gnr", "ok": true, "balance_atomic": "77", "height": 11},
        ]});
        assert!(app.apply_batch(BATCH_SCAN, &batch));
        assert!(!app.apply_batch("sync", &batch));
        assert_eq!(app.sessions[0].height, Some(11));
        assert_eq!(
            app.balance.as_deref(),
            Some("5"),
            "the active wallet scans itself"
        );
        assert_eq!(app.notifier.toasts.len(), 1);
        assert!(app.notifier.toasts[0].text.contains("balance up 1 GNR"));
    }

    #[test]
    fn failure_reasons_are_short_and_plain() {
        assert_eq!(
            unlock_failure_text("wallet decryption failed"),
            "Different password"
        );
        assert_eq!(
            unlock_failure_text("The system cannot find the file specified. (os error 2)"),
            "File missing"
        );
        assert_eq!(
            unlock_failure_text("wallet network mismatch"),
            "Another network"
        );
    }
}
