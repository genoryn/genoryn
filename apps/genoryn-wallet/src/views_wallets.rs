//! The Wallets page: every open and remembered wallet on this computer, richest
//! first, with search, sorting, filters and pages of 100, so 10,000 wallets stay
//! easy to organise. The sorted list is rebuilt only when something changed.
use super::*;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

const PAGE_SIZE: usize = 100;
/// Table columns: rank, wallet, balance, spendable, block, status.
const COLUMNS: [f32; 6] = [36.0, 200.0, 210.0, 140.0, 80.0, 120.0];
/// Room for the Switch, Lock and Scan buttons after the columns.
const ACTIONS_WIDTH: f32 = 210.0;

/// Below this width each wallet takes two lines, so no column or button is
/// ever cut off.
fn compact_below(item_spacing: f32) -> f32 {
    COLUMNS.iter().sum::<f32>() + item_spacing * COLUMNS.len() as f32 + ACTIONS_WIDTH + 40.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum SortBy {
    #[default]
    BalanceHigh,
    BalanceLow,
    Name,
    Block,
}
impl SortBy {
    const ALL: [Self; 4] = [Self::BalanceHigh, Self::BalanceLow, Self::Name, Self::Block];
    fn label(self) -> &'static str {
        match self {
            Self::BalanceHigh => "Balance: highest first",
            Self::BalanceLow => "Balance: lowest first",
            Self::Name => "Name: A to Z",
            Self::Block => "Scanned block: oldest first",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum Filter {
    #[default]
    All,
    Open,
    Locked,
    Attention,
}
impl Filter {
    const ALL: [Self; 4] = [Self::All, Self::Open, Self::Locked, Self::Attention];
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Open => "Open",
            Self::Locked => "Locked",
            Self::Attention => "Need attention",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum State {
    Active,
    Open,
    Queued,
    Locked,
    Failed(String),
}
impl State {
    fn is_open(&self) -> bool {
        matches!(self, Self::Active | Self::Open)
    }
}
#[derive(Clone, Debug)]
pub(super) struct Row {
    pub path: String,
    /// Receiving address of an open wallet; empty while locked.
    pub address: String,
    pub name: String,
    pub balance: Option<u128>,
    pub spendable: Option<u128>,
    pub height: Option<u64>,
    pub state: State,
    pub session: Option<usize>,
}

#[derive(Default)]
pub(super) struct WalletsView {
    search: String,
    sort: SortBy,
    filter: Filter,
    page: usize,
    import_open: bool,
    import_folder: String,
    confirm_forget: Option<String>,
    rows: Vec<Row>,
    /// Counts over every wallet, independent of search and filter.
    totals: Totals,
    signature: Option<u64>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Totals {
    pub open: usize,
    pub locked: usize,
    pub attention: usize,
    pub balance: u128,
    pub spendable: u128,
    pub unscanned: usize,
}

/// Rows matching `search` and `filter`, ordered by `sort`.
pub(super) fn arrange(mut rows: Vec<Row>, search: &str, sort: SortBy, filter: Filter) -> Vec<Row> {
    let needle = search.trim().to_lowercase();
    rows.retain(|row| {
        let shown = match filter {
            Filter::All => true,
            Filter::Open => row.state.is_open(),
            Filter::Locked => matches!(row.state, State::Locked | State::Queued),
            Filter::Attention => matches!(row.state, State::Failed(_)),
        };
        shown
            && (needle.is_empty()
                || row.name.to_lowercase().contains(&needle)
                || row.path.to_lowercase().contains(&needle))
    });
    let by_name = |a: &Row, b: &Row| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    match sort {
        SortBy::BalanceHigh => {
            rows.sort_by(|a, b| sessions::richest_first((a.balance, &a.path), (b.balance, &b.path)))
        }
        SortBy::BalanceLow => rows.sort_by(|a, b| match (a.balance, b.balance) {
            (Some(x), Some(y)) => x.cmp(&y).then_with(|| by_name(a, b)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => by_name(a, b),
        }),
        SortBy::Name => rows.sort_by(|a, b| by_name(a, b).then_with(|| a.path.cmp(&b.path))),
        SortBy::Block => rows.sort_by(|a, b| {
            a.height
                .unwrap_or(0)
                .cmp(&b.height.unwrap_or(0))
                .then_with(|| by_name(a, b))
        }),
    }
    rows
}
fn totals(rows: &[Row]) -> Totals {
    let mut totals = Totals::default();
    let mut seen = HashSet::new();
    for row in rows {
        match &row.state {
            // The same wallet is never counted twice.
            State::Active | State::Open
                if !row.address.is_empty() && !seen.insert(row.address.as_str()) => {}
            State::Active | State::Open => {
                totals.open += 1;
                match row.balance {
                    Some(balance) => totals.balance = totals.balance.saturating_add(balance),
                    None => totals.unscanned += 1,
                }
                totals.spendable = totals.spendable.saturating_add(row.spendable.unwrap_or(0));
            }
            State::Locked | State::Queued => totals.locked += 1,
            State::Failed(_) => {
                totals.locked += 1;
                totals.attention += 1;
            }
        }
    }
    totals
}
/// `*.gnr` files directly inside `folder`, sorted by name.
pub(super) fn wallet_files_in(folder: &str) -> Result<Vec<String>> {
    let mut files: Vec<String> = std::fs::read_dir(folder.trim())
        .with_context(|| format!("cannot read the folder {}", folder.trim()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("gnr"))
        })
        .map(|path| path.display().to_string())
        .collect();
    files.sort_by_key(|path| path.to_lowercase());
    Ok(files)
}

enum Action {
    Switch(usize),
    Lock(usize),
    Rescan(usize),
    UnlockWithPassword(String),
    TryCurrentPassword(String),
    AskForget(String),
    Forget(String),
    UnlockAllLocked,
    Import,
    Add,
}

impl WalletApp {
    /// Every wallet, unsorted, with its state.
    pub(super) fn wallet_rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.sessions.len() + self.preferences.wallets.len() + 1);
        if self.unlocked {
            rows.push(Row {
                path: self.settings.wallet_path.clone(),
                address: self.address.clone(),
                name: sessions::wallet_name(&self.settings.wallet_path),
                balance: sessions::balance_value(self.balance.as_deref()),
                spendable: sessions::balance_value(self.spendable.as_deref()),
                height: self.wallet_height,
                state: State::Active,
                session: None,
            });
        }
        for (index, session) in self.sessions.iter().enumerate() {
            rows.push(Row {
                path: session.path.clone(),
                address: session.address.clone(),
                name: sessions::wallet_name(&session.path),
                balance: sessions::balance_value(session.balance.as_deref()),
                spendable: sessions::balance_value(session.spendable.as_deref()),
                height: session.height,
                state: State::Open,
                session: Some(index),
            });
        }
        let open = self.open_paths();
        let queued: HashSet<String> = self
            .pending_unlocks
            .iter()
            .chain(&self.unlock_inflight)
            .map(|path| sessions::path_key(path))
            .collect();
        for path in &self.preferences.wallets {
            let key = sessions::path_key(path);
            if open.contains(&key) {
                continue;
            }
            let state = if queued.contains(&key) {
                State::Queued
            } else if let Some(reason) = self.unlock_failures.get(&key) {
                State::Failed(reason.clone())
            } else {
                State::Locked
            };
            rows.push(Row {
                path: path.clone(),
                address: String::new(),
                name: sessions::wallet_name(path),
                balance: None,
                spendable: None,
                height: None,
                state,
                session: None,
            });
        }
        rows
    }
    /// Changes whenever anything shown on the Wallets page changes.
    fn wallets_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        let view = &self.wallets_view;
        (view.search.as_str(), view.sort, view.filter).hash(&mut hasher);
        self.unlocked.hash(&mut hasher);
        if self.unlocked {
            self.settings.wallet_path.hash(&mut hasher);
            (&self.balance, &self.spendable, self.wallet_height).hash(&mut hasher);
        }
        for session in &self.sessions {
            (
                &session.path,
                &session.balance,
                &session.spendable,
                session.height,
            )
                .hash(&mut hasher);
        }
        self.preferences.wallets.hash(&mut hasher);
        (
            self.pending_unlocks.len(),
            self.unlock_inflight.len(),
            self.unlock_failures.len(),
        )
            .hash(&mut hasher);
        hasher.finish()
    }
    fn refresh_wallet_rows(&mut self) {
        let signature = self.wallets_signature();
        if self.wallets_view.signature == Some(signature) {
            return;
        }
        let rows = self.wallet_rows();
        let view = &mut self.wallets_view;
        view.totals = totals(&rows);
        view.rows = arrange(rows, &view.search, view.sort, view.filter);
        self.wallets_view.signature = Some(signature);
        let pages = self.wallets_view.rows.len().div_ceil(PAGE_SIZE).max(1);
        self.wallets_view.page = self.wallets_view.page.min(pages - 1);
    }
    pub(super) fn wallets_page(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Wallets",
            "Every wallet on this computer, richest first. Search, sort and organise them here.",
        );
        if !self.unlocked {
            ui.add_space(18.0);
            card(ui, |ui| {
                ui.label("Unlock a wallet first; the other wallets can then be opened with it.");
            });
            return;
        }
        self.refresh_wallet_rows();
        let mut action = None;
        let enabled = self.busy.is_none() && self.seed_backup.is_empty() && !self.process_busy;
        ui.add_space(18.0);
        self.wallets_summary(ui, enabled, &mut action);
        ui.add_space(14.0);
        self.wallets_toolbar(ui);
        ui.add_space(10.0);
        ui.add_enabled_ui(enabled, |ui| self.wallets_table(ui, &mut action));
        self.apply_wallets_action(action);
    }
    fn wallets_summary(&mut self, ui: &mut egui::Ui, enabled: bool, action: &mut Option<Action>) {
        let totals = self.wallets_view.totals;
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 36.0;
                ui.vertical(|ui| {
                    ui.label(RichText::new("TOTAL IN OPEN WALLETS").small().color(GRAY));
                    gnr_amount(ui, Some(&totals.balance.to_string()), 30.0, None);
                    ui.label(
                        RichText::new(format!(
                            "{} GNR spendable now",
                            grouped_amount(&totals.spendable.to_string())
                        ))
                        .small()
                        .color(GRAY),
                    );
                });
                let stat = |ui: &mut egui::Ui, label: &str, value: usize| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(label).small().color(GRAY));
                        ui.label(RichText::new(grouped_count(value)).size(24.0).strong());
                    });
                };
                stat(ui, "OPEN", totals.open);
                stat(ui, "LOCKED", totals.locked);
                if totals.attention > 0 {
                    stat(ui, "NEED ATTENTION", totals.attention);
                }
                if totals.unscanned > 0 {
                    stat(ui, "NOT SCANNED YET", totals.unscanned);
                }
            });
            if let Some((done, total)) = self.unlock_progress {
                ui.add_space(10.0);
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!(
                        "Unlocking wallets: {} of {}",
                        grouped_count(done),
                        grouped_count(total)
                    )),
                );
            }
            ui.add_space(12.0);
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            self.open_wallet_count() < sessions::MAX_OPEN_WALLETS,
                            primary(ui, "Open or create a wallet…"),
                        )
                        .clicked()
                    {
                        *action = Some(Action::Add);
                    }
                    if ui
                        .button(if self.wallets_view.import_open {
                            "Hide folder import"
                        } else {
                            "Import a folder of wallets…"
                        })
                        .clicked()
                    {
                        self.wallets_view.import_open = !self.wallets_view.import_open;
                        if self.wallets_view.import_folder.is_empty() {
                            self.wallets_view.import_folder = self.settings.data_dir.clone();
                        }
                    }
                    let locked = totals.locked;
                    if locked > 0
                        && ui
                            .add_enabled(
                                self.pending_password.is_none(),
                                egui::Button::new(format!(
                                    "Unlock {} locked with this password",
                                    grouped_count(locked)
                                )),
                            )
                            .on_hover_text(
                                "Tries the active wallet's password on every locked wallet. \
                                 Wallets with another password stay locked and are marked.",
                            )
                            .clicked()
                    {
                        *action = Some(Action::UnlockAllLocked);
                    }
                });
            });
            if self.wallets_view.import_open {
                ui.add_space(12.0);
                ui.label("Folder with .gnr wallet files");
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        field(&mut self.wallets_view.import_folder)
                            .desired_width(ui.available_width().min(560.0)),
                    );
                    if ui
                        .add_enabled(
                            enabled && !self.wallets_view.import_folder.trim().is_empty(),
                            egui::Button::new("Add these wallets"),
                        )
                        .clicked()
                    {
                        *action = Some(Action::Import);
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Every .gnr file directly in this folder is added to the list (up to {} wallets in total) \
                         and unlocked with this wallet's password when it matches. Files are never moved or changed.",
                        grouped_count(sessions::MAX_OPEN_WALLETS)
                    ))
                    .small()
                    .color(GRAY),
                );
            }
        });
    }
    fn wallets_toolbar(&mut self, ui: &mut egui::Ui) {
        let view = &mut self.wallets_view;
        let search_width = (ui.available_width() - 280.0).clamp(160.0, 360.0);
        ui.horizontal_wrapped(|ui| {
            let searched = ui
                .add(
                    field(&mut view.search)
                        .hint_text("Search by name or folder")
                        .desired_width(search_width),
                )
                .changed();
            let before = view.sort;
            egui::ComboBox::from_id_salt("wallets_sort")
                .selected_text(view.sort.label())
                .width(200.0)
                .show_ui(ui, |ui| {
                    for sort in SortBy::ALL {
                        ui.selectable_value(&mut view.sort, sort, sort.label());
                    }
                });
            if searched || view.sort != before {
                view.page = 0;
            }
        });
        ui.add_space(4.0);
        let totals = view.totals;
        ui.horizontal_wrapped(|ui| {
            for filter in Filter::ALL {
                let count = match filter {
                    Filter::All => totals.open + totals.locked,
                    Filter::Open => totals.open,
                    Filter::Locked => totals.locked - totals.attention,
                    Filter::Attention => totals.attention,
                };
                if filter == Filter::Attention && count == 0 {
                    continue;
                }
                let label = format!("{} · {}", filter.label(), grouped_count(count));
                if ui.selectable_label(view.filter == filter, label).clicked() {
                    view.filter = filter;
                    view.page = 0;
                }
            }
        });
    }
    fn wallets_table(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let view = &self.wallets_view;
        let count = view.rows.len();
        if count == 0 {
            card(ui, |ui| {
                ui.label(
                    if view.search.trim().is_empty() && view.filter == Filter::All {
                        "No wallets yet."
                    } else {
                        "No wallet matches this search or filter."
                    },
                );
            });
            return;
        }
        let pages = count.div_ceil(PAGE_SIZE);
        let page = view.page.min(pages - 1);
        let first = page * PAGE_SIZE;
        let last = (first + PAGE_SIZE).min(count);
        let compact = ui.available_width() < compact_below(ui.spacing().item_spacing.x);
        let confirm = view.confirm_forget.clone();
        card(ui, |ui| {
            if !compact {
                ui.horizontal(|ui| {
                    let header = |ui: &mut egui::Ui, width: f32, text: &str| {
                        ui.add_sized(
                            [width, 18.0],
                            egui::Label::new(RichText::new(text).small().color(GRAY)),
                        );
                    };
                    for (width, text) in COLUMNS.iter().zip([
                        "#",
                        "WALLET",
                        "BALANCE",
                        "SPENDABLE",
                        "BLOCK",
                        "STATUS",
                    ]) {
                        header(ui, *width, text);
                    }
                });
            }
            for (offset, row) in self.wallets_view.rows[first..last].iter().enumerate() {
                ui.separator();
                wallet_row(
                    ui,
                    first + offset + 1,
                    row,
                    compact,
                    confirm.as_deref(),
                    action,
                );
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Showing {}–{} of {}",
                    grouped_count(first + 1),
                    grouped_count(last),
                    grouped_count(count)
                ))
                .color(GRAY),
            );
            if pages > 1 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(page + 1 < pages, egui::Button::new("Next  →"))
                        .clicked()
                    {
                        self.wallets_view.page = page + 1;
                    }
                    ui.label(format!("Page {} of {}", page + 1, pages));
                    if ui
                        .add_enabled(page > 0, egui::Button::new("←  Previous"))
                        .clicked()
                    {
                        self.wallets_view.page = page - 1;
                    }
                });
            }
        });
    }
    fn apply_wallets_action(&mut self, action: Option<Action>) {
        let Some(action) = action else { return };
        if !matches!(action, Action::AskForget(_)) {
            self.wallets_view.confirm_forget = None;
        }
        match action {
            Action::Switch(index) => self.switch_wallet(index),
            Action::Lock(index) => self.close_session(index),
            Action::Rescan(index) => {
                self.scan_session(index);
                if self.scan_busy.is_none() {
                    self.status = "Another scan is running; try again in a moment.".into();
                }
            }
            Action::UnlockWithPassword(path) => self.begin_unlock_remembered(&path),
            Action::TryCurrentPassword(path) => {
                self.unlock_failures.remove(&sessions::path_key(&path));
                let password = self.password.clone();
                self.queue_unlocks(vec![path], password);
            }
            Action::AskForget(path) => self.wallets_view.confirm_forget = Some(path),
            Action::Forget(path) => self.forget_wallet(&path),
            Action::UnlockAllLocked => {
                let locked = self.remembered_locked();
                for path in &locked {
                    self.unlock_failures.remove(&sessions::path_key(path));
                }
                let password = self.password.clone();
                self.queue_unlocks(locked, password);
            }
            Action::Import => self.import_wallet_folder(),
            Action::Add => self.begin_add_wallet(),
        }
    }
    fn import_wallet_folder(&mut self) {
        let folder = self.wallets_view.import_folder.trim().to_owned();
        let files = match wallet_files_in(&folder) {
            Ok(files) => files,
            Err(error) => {
                self.status = format!("{error:#}");
                return;
            }
        };
        if files.is_empty() {
            self.status = "No .gnr wallet files in this folder.".into();
            return;
        }
        let found = files.len();
        let added = self.preferences.remember_many(files.clone());
        self.save_preferences();
        let open = self.open_paths();
        let to_unlock: Vec<String> = files
            .into_iter()
            .filter(|path| !open.contains(&sessions::path_key(path)))
            .filter(|path| {
                self.preferences
                    .wallets
                    .iter()
                    .any(|known| sessions::same_path(known, path))
            })
            .collect();
        let password = self.password.clone();
        let unlocking = to_unlock.len();
        self.queue_unlocks(to_unlock, password);
        let skipped = found - added;
        self.status = format!(
            "Found {} wallet files: {} added to the list{}. Unlocking {} with this password.",
            grouped_count(found),
            grouped_count(added),
            if skipped > 0 {
                format!(
                    ", {} already listed or over the {} limit",
                    grouped_count(skipped),
                    grouped_count(sessions::MAX_OPEN_WALLETS)
                )
            } else {
                String::new()
            },
            grouped_count(unlocking)
        );
        self.wallets_view.import_open = false;
    }
}

fn status_text(state: &State) -> (String, bool) {
    match state {
        State::Active => ("Active".into(), true),
        State::Open => ("Open".into(), false),
        State::Queued => ("Unlocking…".into(), false),
        State::Locked => ("Locked".into(), false),
        State::Failed(reason) => (reason.clone(), false),
    }
}
fn wallet_row(
    ui: &mut egui::Ui,
    rank: usize,
    row: &Row,
    compact: bool,
    confirm_forget: Option<&str>,
    action: &mut Option<Action>,
) {
    let balance = row.balance.map(|value| value.to_string());
    let (status, strong) = status_text(&row.state);
    let status = if strong {
        RichText::new(status).small().strong()
    } else if matches!(row.state, State::Failed(_)) {
        RichText::new(status).small().color(ERROR)
    } else {
        RichText::new(status).small().color(GRAY)
    };
    let block = row
        .height
        .map(|height| format!("{height}"))
        .unwrap_or_else(|| "—".into());
    let actions = |ui: &mut egui::Ui, action: &mut Option<Action>| {
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| match (&row.state, row.session) {
                (State::Active, _) => {}
                (State::Open, Some(index)) => {
                    if ui.button("Switch").clicked() {
                        *action = Some(Action::Switch(index));
                    }
                    if ui
                        .small_button("Lock")
                        .on_hover_text("Locks this wallet; it stays in the list")
                        .clicked()
                    {
                        *action = Some(Action::Lock(index));
                    }
                    if ui
                        .small_button("Scan")
                        .on_hover_text("Scan this wallet now")
                        .clicked()
                    {
                        *action = Some(Action::Rescan(index));
                    }
                }
                (State::Queued, _) => {}
                _ => {
                    if confirm_forget.is_some_and(|path| path == row.path) {
                        if ui.button("Remove from list").clicked() {
                            *action = Some(Action::Forget(row.path.clone()));
                        }
                        ui.label(RichText::new("The file is kept.").small().color(GRAY));
                    } else {
                        if ui.button("Unlock…").clicked() {
                            *action = Some(Action::UnlockWithPassword(row.path.clone()));
                        }
                        if matches!(row.state, State::Locked)
                            && ui
                                .small_button("Try this password")
                                .on_hover_text("Unlock with the active wallet's password")
                                .clicked()
                        {
                            *action = Some(Action::TryCurrentPassword(row.path.clone()));
                        }
                        if ui
                            .small_button("Remove")
                            .on_hover_text("Remove from this list; the wallet file is not touched")
                            .clicked()
                        {
                            *action = Some(Action::AskForget(row.path.clone()));
                        }
                    }
                }
            },
        );
    };
    let name = RichText::new(&row.name).strong();
    if compact {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{rank}.")).color(GRAY));
            ui.add(egui::Label::new(name).truncate())
                .on_hover_text(&row.path);
            ui.label(status);
        });
        ui.horizontal(|ui| {
            if row.state.is_open() {
                gnr_amount(ui, balance.as_deref(), 15.0, None);
                ui.label(RichText::new(format!("block {block}")).small().color(GRAY));
            }
            actions(ui, action);
        });
        return;
    }
    ui.horizontal(|ui| {
        ui.add_sized(
            [COLUMNS[0], 26.0],
            egui::Label::new(RichText::new(format!("{rank}")).color(GRAY)),
        );
        ui.add_sized([COLUMNS[1], 26.0], egui::Label::new(name).truncate())
            .on_hover_text(&row.path);
        ui.add_sized([COLUMNS[2], 26.0], |ui: &mut egui::Ui| {
            ui.horizontal(|ui| {
                if row.state.is_open() {
                    gnr_amount(ui, balance.as_deref(), 15.0, None);
                } else {
                    ui.label(RichText::new("locked").color(GRAY));
                }
            })
            .response
        });
        let spendable = match (row.state.is_open(), row.spendable) {
            (true, Some(value)) => grouped_amount(&value.to_string()),
            _ => "—".into(),
        };
        ui.add_sized(
            [COLUMNS[3], 26.0],
            egui::Label::new(RichText::new(spendable).color(GRAY)).truncate(),
        );
        ui.add_sized(
            [COLUMNS[4], 26.0],
            egui::Label::new(RichText::new(block).monospace().color(GRAY)),
        );
        ui.add_sized([COLUMNS[5], 26.0], egui::Label::new(status).truncate());
        actions(ui, action);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, balance: Option<u128>, state: State) -> Row {
        Row {
            path: format!("C:/w/{name}.gnr"),
            address: format!("addr-{name}"),
            name: name.into(),
            balance,
            spendable: balance,
            height: balance.map(|b| b as u64),
            state,
            session: None,
        }
    }
    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn richest_first_by_default_with_search_sort_and_filters() {
        let rows = vec![
            row("small", Some(5), State::Open),
            row("locked", None, State::Locked),
            row("big", Some(9_000), State::Active),
            row("broken", None, State::Failed("Different password".into())),
            row("Medium", Some(70), State::Open),
        ];
        let all = arrange(rows.clone(), "", SortBy::default(), Filter::default());
        assert_eq!(names(&all), ["big", "Medium", "small", "broken", "locked"]);
        let low = arrange(rows.clone(), "", SortBy::BalanceLow, Filter::All);
        assert_eq!(names(&low), ["small", "Medium", "big", "broken", "locked"]);
        let by_name = arrange(rows.clone(), "", SortBy::Name, Filter::All);
        assert_eq!(
            names(&by_name),
            ["big", "broken", "locked", "Medium", "small"]
        );
        let open = arrange(rows.clone(), "", SortBy::BalanceHigh, Filter::Open);
        assert_eq!(names(&open), ["big", "Medium", "small"]);
        let locked = arrange(rows.clone(), "", SortBy::BalanceHigh, Filter::Locked);
        assert_eq!(names(&locked), ["locked"]);
        let attention = arrange(rows.clone(), "", SortBy::BalanceHigh, Filter::Attention);
        assert_eq!(names(&attention), ["broken"]);
        let search = arrange(rows.clone(), "  MED ", SortBy::BalanceHigh, Filter::All);
        assert_eq!(names(&search), ["Medium"]);
        let by_folder = arrange(rows, "c:/w/s", SortBy::BalanceHigh, Filter::All);
        assert_eq!(names(&by_folder), ["small"]);
    }

    #[test]
    fn the_same_wallet_is_counted_once_in_totals() {
        let mut copy = row("a", Some(10), State::Open);
        copy.path = "C:/w/a-copy.gnr".into();
        let totals = totals(&[row("a", Some(10), State::Active), copy]);
        assert_eq!((totals.open, totals.balance), (1, 10));
    }

    #[test]
    fn totals_count_only_open_wallets_as_balance() {
        let totals = totals(&[
            row("a", Some(10), State::Active),
            row("b", None, State::Open),
            row("c", None, State::Locked),
            row("d", None, State::Failed("File missing".into())),
            row("e", Some(5), State::Open),
        ]);
        assert_eq!(
            totals,
            Totals {
                open: 3,
                locked: 2,
                attention: 1,
                balance: 15,
                spendable: 15,
                unscanned: 1,
            }
        );
    }

    #[test]
    fn ten_thousand_wallets_arrange_quickly_and_page_by_hundred() {
        let rows: Vec<Row> = (0..sessions::MAX_OPEN_WALLETS)
            .map(|n| {
                row(
                    &format!("wallet-{n:05}"),
                    Some((n as u128 * 7919) % 10_007),
                    State::Open,
                )
            })
            .collect();
        let started = Instant::now();
        let sorted = arrange(rows, "", SortBy::BalanceHigh, Filter::All);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(sorted.len(), sessions::MAX_OPEN_WALLETS);
        assert!(
            sorted
                .windows(2)
                .all(|pair| pair[0].balance >= pair[1].balance)
        );
        assert_eq!(sorted.len().div_ceil(PAGE_SIZE), 100);
    }

    #[test]
    fn the_page_lists_every_wallet_state_and_rebuilds_only_on_change() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.settings.wallet_path = "C:/w/active.gnr".into();
        app.unlocked = true;
        app.balance = Some("5".into());
        app.preferences.remember("C:/w/locked.gnr");
        app.preferences.remember("C:/w/missing.gnr");
        app.preferences.remember("C:/w/active.gnr");
        app.unlock_failures.insert(
            sessions::path_key("C:/w/missing.gnr"),
            "File missing".into(),
        );
        app.refresh_wallet_rows();
        let states: Vec<(String, State)> = app
            .wallets_view
            .rows
            .iter()
            .map(|row| (row.name.clone(), row.state.clone()))
            .collect();
        assert_eq!(
            states,
            [
                ("active".to_owned(), State::Active),
                ("locked".to_owned(), State::Locked),
                ("missing".to_owned(), State::Failed("File missing".into())),
            ]
        );
        let signature = app.wallets_view.signature;
        app.refresh_wallet_rows();
        assert_eq!(app.wallets_view.signature, signature, "nothing changed");
        app.balance = Some("6".into());
        app.refresh_wallet_rows();
        assert_ne!(app.wallets_view.signature, signature);
    }

    #[test]
    fn folder_import_finds_only_wallet_files() {
        let folder = std::env::temp_dir().join(format!(
            "genoryn-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(folder.join("sub.gnr")).unwrap();
        for name in ["b.gnr", "A.GNR", "notes.txt", "c.gnr.chain"] {
            std::fs::write(folder.join(name), b"x").unwrap();
        }
        let files = wallet_files_in(&folder.display().to_string()).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|path| sessions::wallet_name(path))
            .collect();
        assert_eq!(names, ["A", "b"]);
        assert!(wallet_files_in(&folder.join("absent").display().to_string()).is_err());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
