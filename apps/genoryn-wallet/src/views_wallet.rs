//! Native wallet views wallet; uses the shared backend.
use super::*;

const MAX_LISTED_WALLETS: usize = 24;

/// `folder/name.gnr`, adding the extension when the name has none.
pub(super) fn compose_wallet_path(folder: &str, name: &str) -> String {
    let name = name.trim();
    let file = if name.to_ascii_lowercase().ends_with(".gnr") {
        name.to_owned()
    } else {
        format!("{name}.gnr")
    };
    PathBuf::from(folder.trim())
        .join(file)
        .display()
        .to_string()
}
pub(super) fn wallet_name_problem(name: &str) -> Option<GateProblem> {
    let name = name.trim();
    if name.is_empty() {
        return hint("Name the new wallet, for example savings.");
    }
    let stem = name
        .strip_suffix(".gnr")
        .unwrap_or(name)
        .to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if name.chars().count() > 64
        || reserved
        || name.ends_with('.')
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
    {
        return error(
            "Use up to 64 letters, digits, spaces, dots, dashes or underscores for the wallet name.",
        );
    }
    None
}
fn list_wallet_files(folder: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("gnr"))
        })
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    files.sort_by_key(|path| path.to_lowercase());
    files.truncate(MAX_LISTED_WALLETS);
    files
}
#[cfg(target_os = "windows")]
fn existing_directory(path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_dir() {
        Some(path)
    } else {
        path.parent()
            .filter(|parent| parent.is_dir())
            .map(PathBuf::from)
    }
}
#[cfg(target_os = "windows")]
fn pick_wallet_file(current: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Open a GENORYN wallet")
        .add_filter("GENORYN wallet", &["gnr"])
        .add_filter("All files", &["*"]);
    if let Some(directory) = existing_directory(current) {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_file().map(|path| path.display().to_string())
}
#[cfg(target_os = "windows")]
fn pick_folder(current: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_title("Choose where to save the new wallet");
    if let Some(directory) = existing_directory(current) {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_folder().map(|path| path.display().to_string())
}

impl WalletApp {
    fn wallet_folder(&self) -> String {
        PathBuf::from(&self.settings.wallet_path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| self.settings.data_dir.clone())
    }
    /// Wallet files in `folder`, re-read at most every two seconds.
    fn wallet_files_in(&mut self, folder: &str) -> Vec<String> {
        if let Some((cached, read_at, files)) = &self.wallet_files
            && cached == folder
            && read_at.elapsed() < Duration::from_secs(2)
        {
            return files.clone();
        }
        let files = list_wallet_files(folder);
        self.wallet_files = Some((folder.to_owned(), Instant::now(), files.clone()));
        files
    }
    fn first_unopened_wallet(&mut self, folder: &str) -> Option<String> {
        self.wallet_files_in(folder)
            .into_iter()
            .find(|file| !self.is_wallet_open(file))
    }
    /// Points the form at a sensible file for `mode`: an unopened wallet to unlock,
    /// or a free `wallet`, `wallet-2`, … name for a new one.
    pub(super) fn prepare_wallet_form(&mut self, mode: usize) {
        let folder = self.wallet_folder();
        if mode == 0 {
            if (!PathBuf::from(&self.settings.wallet_path).is_file()
                || self.is_wallet_open(&self.settings.wallet_path))
                && let Some(file) = self.first_unopened_wallet(&folder)
            {
                self.settings.wallet_path = file;
            }
            return;
        }
        let name = (1..=999)
            .map(|n| {
                if n == 1 {
                    "wallet".to_owned()
                } else {
                    format!("wallet-{n}")
                }
            })
            .find(|name| {
                let path = compose_wallet_path(&folder, name);
                !PathBuf::from(&path).exists() && !self.is_wallet_open(&path)
            })
            .unwrap_or_else(|| "wallet-new".into());
        self.new_wallet_folder = folder;
        self.new_wallet_name = name;
        self.settings.wallet_path =
            compose_wallet_path(&self.new_wallet_folder, &self.new_wallet_name);
    }
    /// Unlock picks an existing file; create and restore pick a name and a folder.
    fn wallet_file_picker(&mut self, ui: &mut egui::Ui) {
        let enabled = self.busy.is_none();
        if self.wallet_mode == 0 {
            ui.label("Wallet file");
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 120.0).max(200.0);
                ui.add_enabled(
                    enabled,
                    field(&mut self.settings.wallet_path).desired_width(width),
                );
                #[cfg(target_os = "windows")]
                if ui
                    .add_enabled(enabled, egui::Button::new("Browse…"))
                    .clicked()
                    && let Some(path) = pick_wallet_file(&self.settings.wallet_path)
                {
                    self.settings.wallet_path = path;
                }
            });
            let remembered = self
                .preferences
                .wallets
                .iter()
                .filter(|path| PathBuf::from(path).is_file())
                .cloned()
                .collect::<Vec<_>>();
            if !remembered.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("YOUR WALLETS").small().color(GRAY));
                ui.horizontal_wrapped(|ui| {
                    for file in &remembered {
                        let open = self.is_wallet_open(file);
                        let selected = sessions::same_path(file, &self.settings.wallet_path);
                        let mut text = sessions::wallet_name(file);
                        if open {
                            text.push_str(" · open");
                        }
                        if ui
                            .add_enabled(
                                enabled && !open,
                                egui::Button::new(text).selected(selected),
                            )
                            .on_hover_text(file)
                            .clicked()
                        {
                            self.settings.wallet_path = file.clone();
                        }
                    }
                });
            }
            let folder = self.wallet_folder();
            let files = self
                .wallet_files_in(&folder)
                .into_iter()
                .filter(|file| {
                    !remembered
                        .iter()
                        .any(|known| sessions::same_path(known, file))
                })
                .collect::<Vec<_>>();
            if !files.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("OTHER WALLETS IN THIS FOLDER")
                        .small()
                        .color(GRAY),
                );
                ui.horizontal_wrapped(|ui| {
                    for file in files {
                        let open = self.is_wallet_open(&file);
                        let selected = file == self.settings.wallet_path;
                        let mut text = sessions::wallet_name(&file);
                        if open {
                            text.push_str(" · open");
                        }
                        if ui
                            .add_enabled(
                                enabled && !open,
                                egui::Button::new(text).selected(selected),
                            )
                            .on_hover_text(&file)
                            .clicked()
                        {
                            self.settings.wallet_path = file;
                        }
                    }
                });
            }
            return;
        }
        ui.label("Wallet name");
        ui.add_enabled(
            enabled,
            field(&mut self.new_wallet_name)
                .hint_text("savings")
                .char_limit(64)
                .desired_width(480.0),
        );
        ui.add_space(6.0);
        ui.label("Folder");
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 150.0).max(200.0);
            ui.add_enabled(
                enabled,
                field(&mut self.new_wallet_folder).desired_width(width),
            );
            #[cfg(target_os = "windows")]
            if ui
                .add_enabled(enabled, egui::Button::new("Choose folder…"))
                .clicked()
                && let Some(folder) = pick_folder(&self.new_wallet_folder)
            {
                self.new_wallet_folder = folder;
            }
        });
        self.settings.wallet_path =
            compose_wallet_path(&self.new_wallet_folder, &self.new_wallet_name);
        ui.label(
            RichText::new(format!("Saved as {}", self.settings.wallet_path))
                .small()
                .color(GRAY),
        );
    }
    pub(super) fn wallet_gate(&mut self, ui: &mut egui::Ui) -> bool {
        if self.unlocked {
            return true;
        }
        ui.add_space(16.0);
        if self.miner.is_some() {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (badge, _) =
                        ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                    icons::paint(
                        ui.painter(),
                        icons::square(badge.center().x, badge.center().y, 20.0),
                        icons::Icon::Mining,
                        ui.visuals().strong_text_color(),
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Mining continues while the wallet is locked").strong());
                        ui.label(
                            RichText::new("Rewards go to the receiving address chosen when mining started. Mining needs no keys or password; unlock to see the new balance.")
                                .small()
                                .color(GRAY),
                        );
                    });
                });
            });
            ui.add_space(12.0);
        }
        card(ui, |ui| {
            ui.label(RichText::new("Your wallet. Your keys.").size(24.0));
            ui.add_space(8.0);
            ui.label("Create an encrypted wallet, restore your recovery phrase, or unlock an existing file.");
            ui.add_space(18.0);
            if !self.seed_backup.is_empty() {
                ui.label(RichText::new("SAVE YOUR RECOVERY PHRASE").strong());
                ui.label("Anyone with this phrase can spend your funds. Write it on paper, in order. Do not photograph, copy or share it.");
                ui.add_space(8.0);
                recovery::capture_note(ui, self.capture_guard);
                ui.add_space(12.0);
                recovery::words_grid(ui, &self.seed_backup, true);
                ui.add_space(8.0);
                ui.label(
                    RichText::new("You can show these words again later in Settings → Wallet security, with your password.")
                        .small()
                        .color(GRAY),
                );
                ui.add_space(16.0);
                ui.checkbox(
                    &mut self.seed_ack,
                    "I wrote down every word and stored the phrase securely.",
                );
                if ui
                    .add_enabled(
                        self.seed_ack && !self.address.is_empty(),
                        primary(ui, "Continue to wallet").min_size(egui::vec2(200.0, 44.0)),
                    )
                    .clicked()
                {
                    self.seed_backup.zeroize();
                    self.confirm_password.zeroize();
                    self.unlocked = true;
                    self.after_unlock(false);
                    self.request("sync", json!({}));
                }
                return;
            }
            if self.adding_wallet {
                ui.label(
                    RichText::new(format!(
                        "ADDING A WALLET · {} ALREADY OPEN",
                        self.sessions.len()
                    ))
                    .small()
                    .color(GRAY),
                );
                ui.add_space(4.0);
            }
            let mut chosen = None;
            let mut back = false;
            ui.horizontal(|ui| {
                for (index, label) in ["Unlock", "Create", "Restore"].iter().enumerate() {
                    if ui
                        .add_enabled(
                            self.busy.is_none(),
                            egui::Button::new(*label).selected(self.wallet_mode == index),
                        )
                        .clicked()
                    {
                        chosen = Some(index);
                    }
                }
                if self.adding_wallet {
                    ui.add_space(12.0);
                    back = ui
                        .add_enabled(
                            self.busy.is_none(),
                            egui::Button::new("Back to open wallets"),
                        )
                        .clicked();
                }
            });
            if back {
                self.cancel_add_wallet();
                return;
            }
            if let Some(index) = chosen {
                self.wallet_mode = index;
                self.confirm_password.zeroize();
                self.seed_input.zeroize();
                self.prepare_wallet_form(index);
            }
            ui.add_space(14.0);
            self.wallet_file_picker(ui);
            let wallet_exists = PathBuf::from(&self.settings.wallet_path).is_file();
            let already_open = self.is_wallet_open(&self.settings.wallet_path);
            ui.add_space(10.0);
            if self.wallet_mode == 0 {
                ui.label("Password");
            } else {
                ui.label(format!(
                    "Password (at least {MIN_NEW_PASSWORD_CHARS} characters; it alone protects the wallet file)"
                ));
            }
            let password = layout_probe::tag(
                "unlock:password",
                protocol::sensitive_edit(
                    ui,
                    self.busy.is_none(),
                    field(&mut *self.password)
                        .password(true)
                        .desired_width(480.0),
                ),
            );
            // Once per locked session, an existing wallet gets the cursor in its
            // password field, so typing and Enter unlock it.
            let focus_once = egui::Id::new(("unlock-focus", self.lock_epoch));
            if self.wallet_mode == 0
                && wallet_exists
                && self.busy.is_none()
                && !ui.data(|data| data.get_temp::<bool>(focus_once).unwrap_or(false))
                && ui.memory(|memory| memory.focused().is_none())
            {
                ui.data_mut(|data| data.insert_temp(focus_once, true));
                password.request_focus();
                password.scroll_to_me(Some(egui::Align::Center));
            }
            let mut submitted =
                password.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let others = self
                .remembered_locked()
                .into_iter()
                .filter(|path| !sessions::same_path(path, &self.settings.wallet_path))
                .count();
            if self.wallet_mode == 0 && others > 0 {
                ui.checkbox(
                    &mut self.unlock_others,
                    format!(
                        "Also unlock my other {others} remembered wallet{} with this password",
                        if others == 1 { "" } else { "s" }
                    ),
                );
            }
            if self.wallet_mode != 0 {
                recovery::strength_meter(ui, &self.password);
                ui.label("Confirm password");
                submitted |= protocol::sensitive_edit(
                    ui,
                    self.busy.is_none(),
                    field(&mut *self.confirm_password)
                        .password(true)
                        .desired_width(480.0),
                )
                .lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
            }
            if self.wallet_mode == 2 {
                ui.label("Recovery phrase (24 words separated by spaces)");
                protocol::sensitive_edit(
                    ui,
                    self.busy.is_none(),
                    area(&mut *self.seed_input)
                        .password(true)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            }
            ui.add_space(16.0);
            let problem = if already_open {
                error("This wallet is already open. Switch to it from the wallet menu at the top.")
            } else if self.wallet_mode != 0 {
                wallet_name_problem(&self.new_wallet_name)
            } else {
                None
            }
            .or_else(|| {
                gate_problem(
                    self.wallet_mode,
                    wallet_exists,
                    &self.password,
                    &self.confirm_password,
                    &self.seed_input,
                )
            });
            let valid = problem.is_none();
            let label =
                ["Unlock wallet", "Create encrypted wallet", "Restore wallet"][self.wallet_mode];
            if layout_probe::tag(
                "unlock:submit",
                ui.add_enabled(
                    valid && self.busy.is_none(),
                    primary(ui, label).min_size(egui::vec2(220.0, 44.0)),
                ),
            )
            .clicked()
                || (submitted && valid && self.busy.is_none())
            {
                match self.wallet_mode {
                    0 => self.request("open", json!({})),
                    // A new seed starts scanning near the height the node has
                    // verified; offline it scans from genesis.
                    1 => {
                        let birthday = self.node_info.as_ref().map(|node| node.info.height);
                        self.request("create", json!({"birthday_height": birthday}))
                    }
                    _ => {
                        let extra = SecretJson(json!({
                            "seed_phrase": self.seed_input.trim(),
                            "refuse_addresses": self.open_addresses(),
                        }));
                        self.request("restore", extra.0.clone());
                    }
                }
            }
            if let Some(problem) = problem {
                ui.add_space(8.0);
                let text = RichText::new(problem.message);
                ui.label(if problem.blocking {
                    text.color(ERROR)
                } else {
                    text.color(GRAY)
                });
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(
                    "GENORYN mainnet • Unaudited software • Write your 24 words on paper",
                )
                .small()
                .color(GRAY),
            );
        });
        false
    }
    fn all_wallets_card(&mut self, ui: &mut egui::Ui) {
        let (total, unscanned) = self.combined_balance();
        let mut switch_to = None;
        let mut show_all = false;
        let top = self.top_sessions(OVERVIEW_TOP_WALLETS);
        card(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "ALL OPEN WALLETS · {}",
                    grouped_count(self.open_wallet_count())
                ))
                .small()
                .color(GRAY),
            );
            ui.add_space(8.0);
            gnr_amount(ui, Some(&total.to_string()), 34.0, None);
            if unscanned > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} wallet(s) not scanned yet; their balance is added after the first scan.",
                        grouped_count(unscanned)
                    ))
                    .small()
                    .color(GRAY),
                );
            }
            if let Some((done, total)) = self.unlock_progress {
                ui.add_space(4.0);
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!(
                        "Unlocking {} of {}",
                        grouped_count(done),
                        grouped_count(total)
                    )),
                );
            }
            ui.add_space(10.0);
            // Narrow cards (the side column) put the balance under the name.
            let compact = ui.available_width() < 560.0;
            let mut row = |ui: &mut egui::Ui,
                           path: &str,
                           balance: Option<&str>,
                           height: Option<u64>,
                           index: Option<usize>| {
                let scanned = height
                    .map(|height| format!("scanned to block {height}"))
                    .unwrap_or_else(|| "not scanned".into());
                let mut action = |ui: &mut egui::Ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match index {
                            Some(index) => {
                                if ui.button("Switch").clicked() {
                                    switch_to = Some(index);
                                }
                            }
                            None => {
                                ui.label(RichText::new("ACTIVE").small().strong());
                            }
                        }
                    });
                };
                if compact {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(sessions::wallet_name(path)).strong())
                                .truncate(),
                        );
                        action(ui);
                    });
                    ui.horizontal(|ui| {
                        gnr_amount(ui, balance, 15.0, None);
                        ui.label(RichText::new(scanned).small().color(GRAY));
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [190.0, 24.0],
                        egui::Label::new(RichText::new(sessions::wallet_name(path)).strong())
                            .truncate(),
                    );
                    ui.add_sized([230.0, 24.0], |ui: &mut egui::Ui| {
                        ui.horizontal(|ui| gnr_amount(ui, balance, 15.0, None))
                            .response
                    });
                    ui.label(RichText::new(scanned).small().color(GRAY));
                    action(ui);
                });
            };
            row(
                ui,
                &self.settings.wallet_path,
                self.balance.as_deref(),
                self.wallet_height,
                None,
            );
            if self.sessions.len() > top.len() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("TOP {} BY BALANCE", top.len()))
                        .small()
                        .color(GRAY),
                );
            }
            for &index in &top {
                let session = &self.sessions[index];
                row(
                    ui,
                    &session.path,
                    session.balance.as_deref(),
                    session.height,
                    Some(index),
                );
            }
            ui.add_space(10.0);
            let label = if self.sessions.len() > top.len() {
                format!(
                    "Show all {} wallets  →",
                    grouped_count(self.open_wallet_count())
                )
            } else {
                "Manage wallets  →".to_owned()
            };
            show_all = ui.button(label).clicked();
        });
        if let Some(index) = switch_to {
            self.switch_wallet(index);
        } else if show_all {
            self.page = WALLETS_PAGE;
        }
    }
    /// The active wallet comes first. Other open wallets are listed beside it
    /// when the window is wide enough, otherwise below it.
    pub(super) fn overview(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Overview", "Private by design. Finite by protocol.");
        if !self.wallet_gate(ui) {
            return;
        }
        ui.add_space(22.0);
        let width = ui.available_width();
        if self.sessions.is_empty() {
            self.overview_main(ui);
        } else if width >= OVERVIEW_SIDE_BY_SIDE {
            let gap = 20.0;
            let side = (width * 0.32).clamp(300.0, 440.0);
            let main = width - side - gap;
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(main);
                    self.overview_main(ui);
                });
                ui.add_space(gap);
                ui.vertical(|ui| {
                    ui.set_width(side);
                    self.all_wallets_card(ui);
                });
            });
        } else {
            self.overview_main(ui);
            ui.add_space(20.0);
            self.all_wallets_card(ui);
        }
    }
    fn overview_main(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} · WALLET BALANCE",
                    sessions::wallet_name(&self.settings.wallet_path).to_uppercase()
                ))
                .small()
                .color(GRAY),
            );
            if !self.names.mine.is_empty() {
                ui.label(
                    RichText::new(
                        self.names
                            .mine
                            .iter()
                            .map(|name| format!("@{name}"))
                            .collect::<Vec<_>>()
                            .join("  "),
                    )
                    .size(15.0)
                    .strong(),
                );
            }
            ui.add_space(8.0);
            gnr_amount(
                ui,
                self.balance.as_deref(),
                46.0,
                self.balance_is_stale().then_some(GRAY),
            );
            self.balance_state(ui);
            if let Some(name) = self.mining_elsewhere() {
                ui.label(
                    RichText::new(format!(
                        "Mining rewards go to wallet “{name}”, not this one. Switch to it in the wallet menu to see them."
                    ))
                    .color(ERROR),
                );
            }
            ui.add_space(14.0);
            if !self.settings.reference_price.is_empty() {
                let value = self.balance.as_deref().map(|balance| {
                    reference_value::format_usd(balance, &self.settings.reference_price)
                });
                ui.label(match value {
                    Some(Ok(value)) => format!("Reference Value   {value}"),
                    Some(Err(_)) => {
                        "Reference Value unavailable: check the local reference-price setting."
                            .into()
                    }
                    None => "Reference Value unavailable until wallet scan.".into(),
                });
                ui.label(RichText::new("User-entered reference only; not a market quote, account valuation or consensus value.").small().color(GRAY));
            }
            ui.horizontal(|ui| {
                ui.add_sized(
                    [180.0, 22.0],
                    egui::Label::new(RichText::new("Available to spend").color(GRAY)),
                );
                gnr_amount(ui, self.spendable.as_deref(), 15.0, None);
            });
            let pending = self
                .balance
                .as_deref()
                .and_then(|b| b.parse::<u128>().ok())
                .zip(
                    self.spendable
                        .as_deref()
                        .and_then(|b| b.parse::<u128>().ok()),
                )
                .and_then(|(b, s)| b.checked_sub(s));
            ui.horizontal(|ui| {
                ui.add_sized(
                    [180.0, 22.0],
                    egui::Label::new(RichText::new("Immature / unavailable").color(GRAY)),
                );
                gnr_amount(ui, pending.map(|v| v.to_string()).as_deref(), 15.0, None);
            });
            let reserved = self
                .sync_detail
                .as_ref()
                .and_then(|detail| detail["reserved_atomic"].as_str())
                .filter(|amount| *amount != "0")
                .map(str::to_owned);
            if let Some(reserved) = reserved {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [180.0, 22.0],
                        egui::Label::new(RichText::new("Held by pending payments").color(GRAY)),
                    );
                    gnr_amount(ui, Some(&reserved), 15.0, None);
                });
            }
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                if ui.button("Send GNR").clicked() {
                    self.page = 1;
                }
                if ui.button("Receive").clicked() {
                    self.page = 2;
                }
                if ui
                    .add_enabled(self.scan_busy.is_none(), egui::Button::new("Scan chain"))
                    .clicked()
                {
                    self.request("sync", json!({}));
                }
            });
        });
        ui.add_space(20.0);
        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                small_title(ui, "WALLET SCAN");
                ui.label(
                    self.wallet_height
                        .map(|h| format!("Block {h}"))
                        .unwrap_or_else(|| "Not scanned".into()),
                );
                ui.label(
                    RichText::new(
                        self.last_sync
                            .map(|s| {
                                format!(
                                    "Scanned {} s ago · updates automatically with new blocks",
                                    s.elapsed().as_secs()
                                )
                            })
                            .unwrap_or_else(|| {
                                "A local scan reconstructs your private notes.".into()
                            }),
                    )
                    .color(GRAY),
                );
            });
            card(&mut columns[1], |ui| {
                small_title(ui, "NODE CONNECTION");
                ui.label(
                    self.node_info
                        .as_ref()
                        .map(|n| format!("Connected · {} ms", n.latency_ms))
                        .unwrap_or_else(|| "Not connected".into()),
                );
                ui.label(RichText::new(&self.settings.rpc_url).small().color(GRAY));
            });
        });
        ui.add_space(20.0);
        self.mining_overview(ui);
        ui.add_space(20.0);
        ui.label(RichText::new("Balances come from your local wallet scan. Public APIs cannot look up private wallet balances.").color(GRAY));
    }
    pub(super) fn send(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Send",
            "A private payment, built and signed on your computer.",
        );
        if !self.wallet_gate(ui) {
            return;
        }
        ui.add_space(20.0);
        if let Some(txid) = self.last_submission.clone() {
            let (mut prove, mut dismiss) = (false, false);
            card(ui, |ui| {
                ui.label(RichText::new("PAYMENT SENT").small().color(GRAY));
                ui.add(egui::Label::new(RichText::new(&txid).monospace().size(12.5)).wrap());
                ui.label("It is confirmed once a block includes it. To show the recipient (or anyone) that you paid, create a payment proof: a code to paste, or a PDF or JSON file.");
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    prove = ui
                        .add_enabled(self.busy.is_none(), primary(ui, "Create payment proof"))
                        .clicked();
                    dismiss = ui.button("Dismiss").clicked();
                });
            });
            ui.add_space(14.0);
            if prove {
                self.select_proof(&txid);
            } else if dismiss {
                self.last_submission = None;
            }
        }
        card(ui, |ui| {
            ui.label("Recipient address or @username");
            protocol::sensitive_edit(
                ui,
                true,
                field(&mut self.to)
                    .desired_width(f32::INFINITY)
                    .hint_text("@name or gnr1…"),
            );
            if self.to.starts_with("genoryn:") && ui.button("Import payment request").clicked() {
                match payment_uri::parse(&self.to, NETWORK) {
                    Ok(request) => {
                        self.to = request.address;
                        self.amount = request
                            .amount_atomic
                            .map(|a| format_atomic(&a.to_string()))
                            .unwrap_or_default();
                        self.preview = None;
                        self.status="Payment request imported. Review the recipient and amount before sending.".into();
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
            self.recipient_name_hint(ui);
            ui.add_space(14.0);
            ui.columns(2, |columns| {
                columns[0].label("Amount · GNR");
                protocol::sensitive_edit(
                    &mut columns[0],
                    true,
                    field(&mut self.amount)
                        .desired_width(f32::INFINITY)
                        .hint_text("0.00"),
                );
                columns[1].label("Network fee · GNR");
                columns[1].add(field(&mut self.fee).desired_width(f32::INFINITY));
            });
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Fee recommendation");
                for (label, priority) in [
                    ("Economy", genoryn_wallet_core::fees::FeePriority::Economy),
                    ("Standard", genoryn_wallet_core::fees::FeePriority::Standard),
                    ("Priority", genoryn_wallet_core::fees::FeePriority::Priority),
                ] {
                    if ui
                        .add_enabled(self.busy.is_none(), egui::Button::new(label))
                        .clicked()
                    {
                        self.fee = format_atomic(
                            &genoryn_wallet_core::fees::estimate_fee(priority).fee_atomic,
                        );
                        self.preview = None;
                    }
                }
            });
            ui.label(RichText::new("Fixed fee estimate: 1× / 2× / 5× minimum. You may edit the fee. Higher fee per byte improves miner ordering; inclusion time is not guaranteed.").small().color(GRAY));
            ui.add_space(14.0);
            ui.label("Encrypted memo (optional)");
            protocol::sensitive_edit(
                ui,
                true,
                area(&mut self.memo)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(14.0);
            ui.label(RichText::new("Shielded recipient and amount. Network timing and source IP may still reveal information.").color(GRAY));
            ui.add_space(18.0);
            let needed = parse_amount(&self.amount)
                .ok()
                .zip(parse_amount(&self.fee).ok())
                .and_then(|(amount, fee)| amount.checked_add(fee));
            let short = needed.and_then(|needed| self.shortfall(needed));
            let max_single = self.max_single_payment();
            let too_many_notes = short.is_none()
                && (needed
                    .zip(max_single)
                    .is_some_and(|(needed, max)| needed > max)
                    || self
                        .action_error_for("send")
                        .is_some_and(|error| error.contains("more than 16")));
            if too_many_notes {
                self.combine_notes_card(ui, max_single);
            } else if let Some(reason) = short.as_deref().or(self.action_error_for("send")) {
                ui.label(RichText::new(reason).color(ERROR));
                ui.add_space(8.0);
            }
            if ui
                .add_enabled(
                    self.busy.is_none()
                        && self.node_info.is_some()
                        && short.is_none()
                        && !too_many_notes,
                    primary(ui, "Review payment").min_size(egui::vec2(200.0, 44.0)),
                )
                .clicked()
            {
                match (parse_amount(&self.amount), parse_amount(&self.fee)) {
                    (Ok(amount), Ok(fee)) if amount > 0 && !self.to.trim().is_empty() => {
                        if fee < genoryn_core::params::MIN_FEE {
                            self.status = format!(
                                "Fee must be at least {} GNR.",
                                format_atomic(&genoryn_core::params::MIN_FEE.to_string())
                            );
                        } else if self.memo.len() > 480 {
                            self.status = "Memo exceeds 480 UTF-8 bytes.".into();
                        } else if amount.checked_add(fee).is_none() {
                            self.status = "Total amount exceeds supported range.".into();
                        } else {
                            self.preview = Some(PaymentPreview {
                                to: self.to.trim().into(),
                                amount: amount.to_string(),
                                fee: fee.to_string(),
                                memo: self.memo.clone(),
                            });
                        }
                    }
                    _ => {
                        self.status =
                            "Enter a recipient, a positive exact amount, and a valid fee.".into()
                    }
                }
            }
        });
    }
    /// The most one payment can send now: its 16 largest spendable notes.
    fn max_single_payment(&self) -> Option<u64> {
        self.sync_detail.as_ref()?["max_single_payment_atomic"]
            .as_str()?
            .parse()
            .ok()
    }
    /// Why a large payment cannot go out yet, and the one-click way forward.
    fn combine_notes_card(&mut self, ui: &mut egui::Ui, max_single: Option<u64>) {
        let notes = self
            .sync_detail
            .as_ref()
            .and_then(|detail| detail["spendable_notes"].as_u64());
        egui::Frame::new()
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(10.0)
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.label(RichText::new("This amount needs more than 16 of your coins").strong());
                ui.add_space(4.0);
                ui.label(format!(
                    "Your balance is made of separate coins (notes){} — every block reward is one. A single payment can spend at most 16 of them{}.",
                    notes.map(|n| format!(", {n} spendable now")).unwrap_or_default(),
                    max_single
                        .map(|max| format!(", so right now you can send up to {} GNR at once", grouped_amount(&max.to_string())))
                        .unwrap_or_default()
                ));
                ui.label("Combine them first: the wallet sends your coins to yourself, 16 at a time, and only the network fee is spent. After the next block, send this payment again.");
                ui.add_space(8.0);
                let fee = parse_amount(&self.fee)
                    .ok()
                    .filter(|fee| *fee >= genoryn_core::params::MIN_FEE)
                    .unwrap_or(10_000_000);
                if ui
                    .add_enabled(
                        self.busy.is_none() && self.node_info.is_some(),
                        primary(ui, "Combine notes now"),
                    )
                    .clicked()
                {
                    self.action_error = None;
                    self.request("combine_notes", json!({"fee_atomic": fee.to_string()}));
                }
                if let Some(error) = self.action_error_for("combine_notes") {
                    ui.label(RichText::new(error).color(ERROR));
                }
            });
        ui.add_space(8.0);
    }
    pub(super) fn receive(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Receive",
            "Share your receiving identity. Keep your recovery phrase private.",
        );
        if !self.wallet_gate(ui) {
            return;
        }
        ui.add_space(20.0);
        card(ui, |ui| {
            ui.horizontal_top(|ui| {
                draw_qr(
                    ui,
                    self.payment_request.as_deref().unwrap_or(&self.address),
                    200.0,
                );
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    small_title(ui, "RECEIVING ADDRESS");
                    ui.add(
                        area(&mut self.address)
                            .interactive(false)
                            .desired_width(500.0)
                            .desired_rows(4),
                    );
                    if ui.button("Copy receiving address").clicked() {
                        ui.ctx().copy_text(self.address.clone());
                        self.clipboard_warning = Some(Instant::now());
                    }
                    ui.label(
                        RichText::new(if self.payment_request.is_some() {
                            "The QR contains a versioned payment request."
                        } else {
                            "The QR contains your raw receiving address."
                        })
                        .small()
                        .color(GRAY),
                    );
                });
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label("Request amount · GNR (optional)");
                protocol::sensitive_edit(
                    ui,
                    true,
                    field(&mut self.request_amount).desired_width(200.0),
                );
                if ui.button("Create payment request").clicked() {
                    match payment_uri::build(
                        NETWORK,
                        &self.address,
                        if self.request_amount.is_empty() {
                            None
                        } else {
                            Some(self.request_amount.as_str())
                        },
                    ) {
                        Ok(uri) => self.payment_request = Some(uri),
                        Err(error) => self.status = error.to_string(),
                    }
                }
            });
            if let Some(uri) = &self.payment_request {
                ui.label(RichText::new(uri).monospace().small());
                if ui.button("Copy payment request").clicked() {
                    ui.ctx().copy_text(uri.clone());
                    self.clipboard_warning = Some(Instant::now());
                }
            }
        });
        ui.add_space(18.0);
        card(ui, |ui| self.names_section(ui));
    }
    pub(super) fn transactions(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Transactions",
            "Your own locally reconstructed private history.",
        );
        ui.add_space(12.0);
        if !self.wallet_gate(ui) {
            return;
        }
        ui.add_space(20.0);
        if let Some(txid) = &self.last_submission {
            ui.label(format!("Last submitted payment: {txid}"));
            ui.label("A submission is not a confirmation. Scan mined blocks to check its state.");
            ui.add_space(12.0);
        }
        self.outbox_card(ui);
        let target = self.settings.confirmation_target;
        let transfers = self
            .history
            .iter()
            .filter(|item| category(item) == Category::Transfers)
            .count();
        let mined = self.rewards().len();
        ui.horizontal(|ui| {
            for (mined_tab, label) in [
                (false, format!("Transfers  {transfers}")),
                (true, format!("Mined  {mined}")),
            ] {
                if ui
                    .add_sized(
                        [150.0, 36.0],
                        egui::Button::new(RichText::new(label).size(15.0))
                            .selected(self.tx_view.mined == mined_tab),
                    )
                    .clicked()
                    && self.tx_view.mined != mined_tab
                {
                    self.tx_view.mined = mined_tab;
                    self.tx_view.limit = 0;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.scan_busy.is_none(),
                        egui::Button::new("Rescan history"),
                    )
                    .on_hover_text("Rebuild this wallet's history from the first block")
                    .clicked()
                {
                    self.request("sync", json!({"full_rescan": true}));
                }
            });
        });
        ui.add_space(10.0);
        if self.tx_view.mined {
            if self.last_sync.is_none() {
                ui.label("No wallet scan has completed yet.");
            } else {
                self.mined_rewards(ui);
            }
            return;
        }
        ui.horizontal(|ui| {
            let search = ui.add(
                field(&mut self.tx_view.search)
                    .hint_text("Search transaction ID, memo, amount or block")
                    .char_limit(128)
                    .desired_width(420.0),
            );
            if search.changed() {
                self.tx_view.limit = 0;
            }
            if !self.tx_view.mined {
                egui::ComboBox::from_id_salt("tx_direction")
                    .selected_text(self.tx_view.direction.label())
                    .show_ui(ui, |ui| {
                        for direction in [Direction::All, Direction::Received, Direction::Sent] {
                            ui.selectable_value(
                                &mut self.tx_view.direction,
                                direction,
                                direction.label(),
                            );
                        }
                    });
            }
            egui::ComboBox::from_id_salt("tx_status")
                .selected_text(self.tx_view.status.label())
                .show_ui(ui, |ui| {
                    for status in [
                        StatusFilter::All,
                        StatusFilter::Confirmed,
                        StatusFilter::Pending,
                    ] {
                        ui.selectable_value(&mut self.tx_view.status, status, status.label());
                    }
                });
            if !self.tx_view.search.is_empty() && ui.button("Clear").clicked() {
                self.tx_view.search.clear();
            }
        });
        ui.add_space(14.0);
        if self.last_sync.is_none() {
            ui.label("No wallet scan has completed yet.");
            return;
        }
        let matching = self
            .history
            .iter()
            .filter(|item| matches_view(item, &self.tx_view, target))
            .collect::<Vec<_>>();
        if matching.is_empty() {
            card(ui, |ui| {
                ui.label(
                    RichText::new(if self.history.is_empty() {
                        "No wallet transactions were found in the scanned chain."
                    } else if self.tx_view.mined {
                        "No mined rewards match these filters."
                    } else {
                        "No transfers match these filters. Mining rewards are listed under Mined."
                    })
                    .color(GRAY),
                );
            });
            return;
        }
        let limit = self.tx_view.limit.max(HISTORY_PAGE);
        let mut export = None;
        let busy = self.busy.is_some();
        for item in matching.iter().take(limit) {
            if let Some(txid) = history_row(ui, item, target, busy) {
                export = Some(txid);
            }
            ui.add_space(6.0);
        }
        if matching.len() > limit {
            ui.add_space(6.0);
            if ui
                .button(format!("Show more ({} remaining)", matching.len() - limit))
                .clicked()
            {
                self.tx_view.limit = limit + HISTORY_PAGE;
            }
        }
        if let Some(txid) = export {
            self.select_proof(&txid);
        }
    }
}

const HISTORY_PAGE: usize = 50;
/// Content width from which the Overview shows open wallets beside the active one.
const OVERVIEW_SIDE_BY_SIDE: f32 = 1060.0;
/// Background wallets listed on the Overview; the Wallets page lists all.
const OVERVIEW_TOP_WALLETS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    Transfers,
    Mined,
}
fn category(item: &Value) -> Category {
    match item["direction"].as_str() {
        Some("mined" | "treasury") => Category::Mined,
        _ => Category::Transfers,
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Direction {
    #[default]
    All,
    Received,
    Sent,
}
impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All transfers",
            Self::Received => "Received",
            Self::Sent => "Sent",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum StatusFilter {
    #[default]
    All,
    Confirmed,
    Pending,
}
impl StatusFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "Any status",
            Self::Confirmed => "Confirmed",
            Self::Pending => "Awaiting confirmations",
        }
    }
}
/// Transactions page state; transfers are the default category.
#[derive(Default)]
pub(super) struct HistoryView {
    pub mined: bool,
    pub rewards: views_status::RewardFilter,
    pub search: String,
    pub direction: Direction,
    pub status: StatusFilter,
    pub limit: usize,
}
fn matches_view(item: &Value, view: &HistoryView, confirmation_target: u64) -> bool {
    let wanted = if view.mined {
        Category::Mined
    } else {
        Category::Transfers
    };
    if category(item) != wanted {
        return false;
    }
    let direction = item["direction"].as_str().unwrap_or_default();
    if !view.mined
        && match view.direction {
            Direction::All => false,
            Direction::Received => direction != "received",
            Direction::Sent => direction != "sent",
        }
    {
        return false;
    }
    let confirmations = item["confirmations"].as_u64().unwrap_or(0);
    let confirmed = confirmations >= confirmation_target;
    if match view.status {
        StatusFilter::All => false,
        StatusFilter::Confirmed => !confirmed,
        StatusFilter::Pending => confirmed,
    } {
        return false;
    }
    let query = view.search.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let block = query
        .trim_start_matches("block")
        .trim()
        .trim_start_matches('#');
    let amount = item["amount_atomic"]
        .as_str()
        .map(format_atomic)
        .unwrap_or_default();
    item["txid"]
        .as_str()
        .is_some_and(|txid| txid.contains(&query))
        || item["memo"]
            .as_str()
            .is_some_and(|memo| memo.to_lowercase().contains(&query))
        || amount.starts_with(&query)
        || direction.contains(&query)
        || item["height"]
            .as_u64()
            .is_some_and(|height| block.parse::<u64>().ok() == Some(height))
}
/// `YYYY-MM-DD HH:MM UTC` from Unix seconds (proleptic Gregorian calendar).
pub(super) fn format_utc(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds / 3_600,
        seconds % 3_600 / 60
    )
}
/// One history entry; returns the transaction ID when its proof export is requested.
fn history_row(
    ui: &mut egui::Ui,
    item: &Value,
    confirmation_target: u64,
    busy: bool,
) -> Option<String> {
    let direction = item["direction"].as_str().unwrap_or("transaction");
    let (icon, title, sign) = match direction {
        "received" => (icons::Icon::Receive, "Received", "+"),
        "sent" => (icons::Icon::Send, "Sent", "−"),
        "mined" => (icons::Icon::Mining, "Mined block reward", "+"),
        "treasury" => (icons::Icon::Mining, "Treasury issuance", "+"),
        "name" => (icons::Icon::Network, "Username", "−"),
        _ => (icons::Icon::Transactions, "Transaction", ""),
    };
    let confirmations = item["confirmations"].as_u64().unwrap_or(0);
    let txid = item["txid"].as_str().unwrap_or_default();
    let mut export = None;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let dark = ui.visuals().dark_mode;
            let ink = if dark { Color32::WHITE } else { Color32::BLACK };
            let (badge, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
            ui.painter().circle_filled(
                badge.center(),
                19.0,
                Color32::from_gray(if dark { 32 } else { 228 }),
            );
            icons::paint(
                ui.painter(),
                icons::square(badge.center().x, badge.center().y, 18.0),
                icon,
                ink,
            );
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).size(16.0).strong());
                let mut details = Vec::new();
                if let Some(height) = item["height"].as_u64() {
                    details.push(format!("Block {height}"));
                }
                if let Some(timestamp) = item["timestamp"].as_u64() {
                    details.push(format_utc(timestamp));
                }
                details.push(if confirmations >= confirmation_target {
                    format!("{confirmations} confirmations")
                } else {
                    format!("{confirmations} of {confirmation_target} confirmations")
                });
                ui.label(RichText::new(details.join("  ·  ")).small().color(GRAY));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.horizontal(|ui| {
                    gnr_amount(ui, item["amount_atomic"].as_str(), 19.0, None);
                    ui.label(RichText::new(sign).size(19.0));
                });
            });
        });
        ui.add_space(4.0);
        if let Some(memo) = item["memo"].as_str().filter(|memo| !memo.is_empty()) {
            ui.label(format!("Memo: {memo}"));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(txid).monospace().small().color(GRAY));
            if !txid.is_empty() && ui.small_button("Copy ID").clicked() {
                ui.ctx().copy_text(txid.to_owned());
            }
            if !txid.is_empty()
                && ui
                    .add_enabled(!busy, egui::Button::new("Export payment proof").small())
                    .clicked()
            {
                export = Some(txid.to_owned());
            }
        });
    });
    export
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct GateProblem {
    pub message: String,
    /// False while a field is merely unfilled; true for input that must be corrected.
    pub blocking: bool,
}
fn hint(message: impl Into<String>) -> Option<GateProblem> {
    Some(GateProblem {
        message: message.into(),
        blocking: false,
    })
}
fn error(message: impl Into<String>) -> Option<GateProblem> {
    Some(GateProblem {
        message: message.into(),
        blocking: true,
    })
}
/// Explains why the unlock/create/restore action is unavailable; `None` enables it.
pub(super) fn gate_problem(
    mode: usize,
    wallet_exists: bool,
    password: &str,
    confirm: &str,
    seed: &str,
) -> Option<GateProblem> {
    if mode == 0 {
        if !wallet_exists {
            return hint(
                "No wallet file exists at this path yet. Choose Create for a new wallet or Restore to recover one from its 24 words.",
            );
        }
        if password.is_empty() {
            return hint("Enter the wallet password.");
        }
        return None;
    }
    if wallet_exists {
        return error(
            "A wallet file already exists at this path. Use Unlock, or enter a different file name for the new wallet.",
        );
    }
    if password.is_empty() {
        return hint(format!(
            "Choose a password of at least {MIN_NEW_PASSWORD_CHARS} characters."
        ));
    }
    if let Some(problem) = recovery::new_password_problem(password) {
        return error(problem);
    }
    if confirm.is_empty() {
        return hint("Repeat the same password in Confirm password.");
    }
    if confirm != password {
        let still_typing = confirm.len() < password.len() && password.starts_with(confirm);
        return if still_typing {
            hint("Repeat the same password in Confirm password.")
        } else {
            error("The two passwords do not match.")
        };
    }
    if mode == 2 {
        let words = seed.split_whitespace().count();
        if words == 0 {
            return hint(format!("Enter your {RECOVERY_WORDS}-word recovery phrase."));
        }
        if words != RECOVERY_WORDS {
            return error(format!(
                "The recovery phrase has {words} words; exactly {RECOVERY_WORDS} are required."
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "correct-horse";
    const SEED: &str = "a a a a a a a a a a a a a a a a a a a a a a a a";

    fn blocking(problem: Option<GateProblem>) -> Option<bool> {
        problem.map(|p| p.blocking)
    }

    #[test]
    fn unlock_requires_an_existing_file_and_a_password() {
        assert_eq!(blocking(gate_problem(0, false, GOOD, "", "")), Some(false));
        assert_eq!(blocking(gate_problem(0, true, "", "", "")), Some(false));
        assert_eq!(gate_problem(0, true, "short", "", ""), None);
    }

    #[test]
    fn create_refuses_to_target_an_existing_file() {
        assert_eq!(blocking(gate_problem(1, true, GOOD, GOOD, "")), Some(true));
        assert_eq!(gate_problem(1, false, GOOD, GOOD, ""), None);
    }

    #[test]
    fn new_password_length_counts_characters_not_bytes() {
        let multibyte = "€€€€";
        assert!(multibyte.len() >= MIN_NEW_PASSWORD_CHARS);
        assert_eq!(
            blocking(gate_problem(1, false, multibyte, multibyte, "")),
            Some(true)
        );
        let problem = gate_problem(1, false, "123456789", "123456789", "").unwrap();
        assert!(problem.message.contains("9 of at least 12"));
        let easy = gate_problem(1, false, "aaaaaaaaaaaaaa", "aaaaaaaaaaaaaa", "").unwrap();
        assert!(easy.blocking && easy.message.contains("easy to guess"));
    }

    #[test]
    fn confirmation_mismatch_is_reported_only_once_it_diverges() {
        assert_eq!(blocking(gate_problem(1, false, GOOD, "", "")), Some(false));
        assert_eq!(
            blocking(gate_problem(1, false, GOOD, "correct", "")),
            Some(false)
        );
        assert_eq!(
            blocking(gate_problem(1, false, GOOD, "wrong", "")),
            Some(true)
        );
        assert_eq!(
            blocking(gate_problem(1, false, GOOD, "correct-horse!", "")),
            Some(true)
        );
    }

    fn entry(direction: &str, amount: &str, height: u64, confirmations: u64, memo: &str) -> Value {
        json!({
            "txid": format!("{direction}{height:04}abcdef"),
            "direction": direction,
            "amount_atomic": amount,
            "height": height,
            "timestamp": 0,
            "status": "",
            "confirmations": confirmations,
            "memo": memo,
        })
    }

    #[test]
    fn history_splits_transfers_from_mining_and_filters() {
        let received = entry("received", "1500000000000", 12, 20, "Rent for March");
        let sent = entry("sent", "250000000000", 15, 3, "");
        let mined = entry("mined", "6920000000000", 16, 2, "");
        let treasury = entry("treasury", "1000000000000", 16, 2, "");
        let mut view = HistoryView::default();
        assert!(!view.mined, "transfers are the default category");
        assert!(matches_view(&received, &view, 10) && matches_view(&sent, &view, 10));
        assert!(!matches_view(&mined, &view, 10) && !matches_view(&treasury, &view, 10));
        view.direction = Direction::Sent;
        assert!(!matches_view(&received, &view, 10) && matches_view(&sent, &view, 10));
        view.direction = Direction::All;
        view.status = StatusFilter::Confirmed;
        assert!(matches_view(&received, &view, 10) && !matches_view(&sent, &view, 10));
        view.status = StatusFilter::Pending;
        assert!(!matches_view(&received, &view, 10) && matches_view(&sent, &view, 10));
        view.status = StatusFilter::All;
        view.mined = true;
        view.direction = Direction::Sent;
        assert!(
            matches_view(&mined, &view, 10),
            "direction applies to transfers only"
        );
        assert!(matches_view(&treasury, &view, 10) && !matches_view(&received, &view, 10));
    }

    #[test]
    fn search_matches_ids_memos_amounts_and_blocks() {
        let received = entry("received", "1500000000000", 12, 20, "Rent for March");
        let mut view = HistoryView::default();
        for query in [
            "received0012",
            "rent",
            "MARCH",
            "1.5",
            "block 12",
            "#12",
            "12",
        ] {
            view.search = query.into();
            assert!(matches_view(&received, &view, 10), "{query}");
        }
        for query in ["zzz", "block 13", "2.5", "sent"] {
            view.search = query.into();
            assert!(!matches_view(&received, &view, 10), "{query}");
        }
    }

    #[test]
    fn utc_dates_are_exact() {
        assert_eq!(format_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(format_utc(1_791_582_544), "2026-10-09 21:49 UTC");
    }

    #[test]
    fn wallet_names_are_safe_file_names() {
        assert_eq!(
            compose_wallet_path("folder", "savings"),
            PathBuf::from("folder")
                .join("savings.gnr")
                .display()
                .to_string()
        );
        assert!(compose_wallet_path("folder", " Cold.GNR ").ends_with("Cold.GNR"));
        assert_eq!(wallet_name_problem("savings 2026_v1.backup"), None);
        assert!(wallet_name_problem("").is_some_and(|problem| !problem.blocking));
        for bad in [
            "a/b", "..\\x", "con", "COM1", "lpt9.gnr", "name.", "a:b", "x*",
        ] {
            assert!(
                wallet_name_problem(bad).is_some_and(|problem| problem.blocking),
                "{bad}"
            );
        }
    }

    #[test]
    fn restore_requires_exactly_24_words() {
        assert_eq!(
            blocking(gate_problem(2, false, GOOD, GOOD, " ")),
            Some(false)
        );
        let twelve = "a a a a a a a a a a a a";
        let problem = gate_problem(2, false, GOOD, GOOD, twelve).unwrap();
        assert!(problem.blocking && problem.message.contains("12 words"));
        assert_eq!(gate_problem(2, false, GOOD, GOOD, SEED), None);
        assert_eq!(
            gate_problem(
                2,
                false,
                GOOD,
                GOOD,
                &format!("  {}\n", SEED.replace(' ', "  "))
            ),
            None
        );
    }
}
