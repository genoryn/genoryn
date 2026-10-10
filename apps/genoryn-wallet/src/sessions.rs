//! Several unlocked wallets in one window. The active wallet lives in the app's
//! own fields; the others wait here with their last scan. Lock closes all of them.
use super::*;

pub(super) const MAX_OPEN_WALLETS: usize = 8;
const BACKGROUND_SYNC: &str = "sync@";
const BACKGROUND_UNLOCK: &str = "unlock@";

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

/// The file name without its extension, as shown in the wallet switcher.
pub(super) fn wallet_name(path: &str) -> String {
    PathBuf::from(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Wallet".into())
}
pub(super) fn same_path(a: &str, b: &str) -> bool {
    let normalize = |path: &str| {
        let path = path.trim().replace('\\', "/");
        if cfg!(windows) {
            path.to_lowercase()
        } else {
            path
        }
    };
    normalize(a) == normalize(b)
}
pub(super) fn sum_balances<'a>(balances: impl Iterator<Item = Option<&'a str>>) -> (u128, usize) {
    let mut total = 0u128;
    let mut unknown = 0;
    for balance in balances {
        match balance.and_then(|value| value.parse::<u128>().ok()) {
            Some(value) => total = total.saturating_add(value),
            None => unknown += 1,
        }
    }
    (total, unknown)
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
    pub(super) fn is_wallet_open(&self, path: &str) -> bool {
        self.sessions
            .iter()
            .any(|session| same_path(&session.path, path))
            || (self.unlocked && same_path(&self.settings.wallet_path, path))
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
            self.status = format!("At most {MAX_OPEN_WALLETS} wallets can be open at once.");
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
    /// Scans an inactive wallet; its result updates only that wallet's summary.
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
    /// Remembered wallets that exist on disk but are not open now.
    pub(super) fn remembered_locked(&self) -> Vec<String> {
        self.preferences
            .wallets
            .iter()
            .filter(|path| PathBuf::from(path).is_file() && !self.is_wallet_open(path))
            .cloned()
            .collect()
    }
    /// After the active wallet unlocks: remember it and, if chosen, try the same
    /// password on the other remembered wallets.
    pub(super) fn after_unlock(&mut self, try_others: bool) {
        self.preferences.remember(&self.settings.wallet_path);
        self.save_preferences();
        if try_others && self.unlock_others {
            let others = self.remembered_locked();
            if !others.is_empty() {
                self.pending_unlocks = others;
                self.pending_password = Some(self.password.clone());
            }
        }
    }
    /// Opens the next queued remembered wallet once no other CLI work is running.
    pub(super) fn process_pending_unlocks(&mut self) {
        if self.busy.is_some() || self.unlock_inflight.is_some() || !self.unlocked {
            return;
        }
        let Some(password) = self.pending_password.clone() else {
            self.pending_unlocks.clear();
            return;
        };
        while !self.pending_unlocks.is_empty() {
            let path = self.pending_unlocks.remove(0);
            if self.is_wallet_open(&path) || self.open_wallet_count() >= MAX_OPEN_WALLETS {
                continue;
            }
            self.unlock_inflight = Some((path.clone(), password.clone()));
            let label = format!("{BACKGROUND_UNLOCK}{path}");
            self.dispatch("open", label, Some((path, password)), json!({}));
            if self.busy.is_none() {
                self.unlock_inflight = None;
                self.pending_unlocks.clear();
                self.pending_password = None;
            }
            return;
        }
        self.pending_password = None;
    }
    /// Turns a successful background unlock into an open wallet.
    pub(super) fn apply_background_unlock(&mut self, label: &str, result: &Value) -> bool {
        let Some(path) = label.strip_prefix(BACKGROUND_UNLOCK) else {
            return false;
        };
        if let Some((pending, password)) = self.unlock_inflight.take()
            && same_path(&pending, path)
            && !self.is_wallet_open(path)
        {
            self.sessions.push(WalletSession {
                path: pending,
                password,
                address: result["address"].as_str().unwrap_or_default().to_owned(),
                balance: None,
                spendable: None,
                height: None,
                history: Vec::new(),
                detail: None,
                last_sync: None,
            });
            self.status = format!("Also unlocked {}.", wallet_name(path));
        }
        true
    }
    /// A remembered wallet with another password stays in the menu to unlock.
    pub(super) fn background_unlock_failed(&mut self, label: &str) -> bool {
        let Some(path) = label.strip_prefix(BACKGROUND_UNLOCK) else {
            return false;
        };
        self.unlock_inflight = None;
        self.status = format!(
            "{} has a different password. Unlock it from the wallet menu at the top.",
            wallet_name(path)
        );
        true
    }
    /// A background unlock whose result was discarded is tried again.
    pub(super) fn requeue_stale_unlock(&mut self, label: &str) {
        if label.starts_with(BACKGROUND_UNLOCK)
            && let Some((path, _)) = self.unlock_inflight.take()
        {
            self.pending_unlocks.insert(0, path);
        }
    }
    pub(super) fn begin_unlock_remembered(&mut self, path: &str) {
        if self.unlocked {
            self.begin_add_wallet();
            if !self.adding_wallet {
                return;
            }
        }
        self.wallet_mode = 0;
        self.settings.wallet_path = path.to_owned();
        self.password.zeroize();
    }
    /// The top-bar wallet menu: switch, add another wallet, or close the active one.
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
            Unlock(String),
            Add,
            Close,
        }
        let locked = self.remembered_locked();
        let mut action = None;
        let enabled = self.busy.is_none() && self.seed_backup.is_empty() && !self.process_busy;
        ui.add_enabled_ui(enabled, |ui| {
            egui::ComboBox::from_id_salt("wallet_switcher")
                .selected_text(RichText::new(title).strong())
                .width(240.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(300.0);
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
                    if !self.sessions.is_empty() {
                        ui.label(RichText::new("SWITCH TO").small().color(GRAY));
                    }
                    for (index, session) in self.sessions.iter().enumerate() {
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
                    if !locked.is_empty() {
                        ui.separator();
                        ui.label(RichText::new("LOCKED").small().color(GRAY));
                        for path in &locked {
                            if ui
                                .selectable_label(false, format!("{}   Unlock…", wallet_name(path)))
                                .on_hover_text(path)
                                .clicked()
                            {
                                action = Some(Action::Unlock(path.clone()));
                            }
                        }
                    }
                    ui.separator();
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
            Some(Action::Unlock(path)) => self.begin_unlock_remembered(&path),
            Some(Action::Add) => self.begin_add_wallet(),
            Some(Action::Close) => self.close_active_wallet(),
            None => {}
        }
    }
    /// The inactive wallet furthest behind the node, if any is behind.
    pub(super) fn session_needing_scan(&self, node_height: u64) -> Option<usize> {
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| session.height.is_none_or(|height| height < node_height))
            .min_by_key(|(_, session)| session.height.unwrap_or(0))
            .map(|(index, _)| index)
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
        app.lock();
        assert!(
            app.sessions.is_empty() && !app.unlocked,
            "lock closes every wallet"
        );
    }

    #[test]
    fn remembered_wallets_unlock_with_one_password_or_wait_in_the_menu() {
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
        let file = |name: &str| {
            let path = folder.join(name);
            std::fs::write(&path, b"encrypted").unwrap();
            path.display().to_string()
        };
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
        assert_eq!(
            app.pending_password.as_deref().map(String::as_str),
            Some("active-password")
        );

        app.unlock_inflight = Some((two.clone(), Zeroizing::new("active-password".into())));
        let label = format!("{BACKGROUND_UNLOCK}{two}");
        assert!(app.apply_background_unlock(&label, &json!({"address": "gnrd-two"})));
        assert!(app.is_wallet_open(&two));
        assert_eq!(app.sessions[0].password.as_str(), "active-password");
        assert_eq!(app.sessions[0].address, "gnrd-two");

        app.unlock_inflight = Some((three.clone(), Zeroizing::new("wrong".into())));
        assert!(app.background_unlock_failed(&format!("{BACKGROUND_UNLOCK}{three}")));
        assert!(app.unlock_inflight.is_none() && !app.is_wallet_open(&three));
        assert_eq!(
            app.remembered_locked(),
            std::slice::from_ref(&three),
            "it waits in the menu"
        );

        app.unlock_inflight = Some((three.clone(), Zeroizing::new("x".into())));
        app.requeue_stale_unlock(&format!("{BACKGROUND_UNLOCK}{three}"));
        assert_eq!(
            app.pending_unlocks[0], three,
            "a discarded result is retried"
        );

        app.process_pending_unlocks();
        assert!(app.unlock_inflight.is_none() && app.pending_unlocks.is_empty());
        assert!(
            app.pending_password.is_none(),
            "a CLI that cannot start ends the queue"
        );

        app.close_active_wallet();
        assert!(
            !app.preferences.wallets.contains(&one),
            "closing forgets the wallet"
        );
        app.lock();
        assert!(app.pending_password.is_none() && app.sessions.is_empty());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn background_results_update_only_their_own_wallet() {
        let ctx = egui::Context::default();
        let mut app = unlocked_app(&ctx, "one.gnr", "5");
        app.sessions.push(session("two.gnr", None));
        assert_eq!(app.session_needing_scan(10), Some(0));
        let result =
            json!({"balance_atomic": "9", "spendable_atomic": "8", "height": 10, "history": []});
        assert!(app.apply_background_sync("sync@two.gnr", &result));
        assert!(!app.apply_background_sync("sync", &result));
        assert_eq!(app.sessions[0].balance.as_deref(), Some("9"));
        assert_eq!(app.balance.as_deref(), Some("5"));
        assert_eq!(app.session_needing_scan(10), None);
    }
}
