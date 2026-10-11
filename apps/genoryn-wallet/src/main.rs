#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod capture;
mod controller;
mod icons;
mod layout_probe;
mod miner_recovery;
mod motion;
mod names_ui;
mod network_privacy;
mod network_transition;
mod node_controls;
mod notifications;
mod payment_proof;
mod payment_uri;
mod preferences;
mod proof_challenge;
mod proof_formats;
mod protocol;
mod recovery;
mod reference_value;
mod sessions;
mod updates;
mod views_node;
mod views_security;
mod views_status;
mod views_wallet;
mod views_wallets;
use anyhow::{Context, Result, ensure};
use eframe::egui::{self, Color32, RichText, Stroke};
use genoryn_connect::{Connect, ConnectConfig, HealthyNode};
use genoryn_core::params::Network;
use protocol::{OwnedChild, SecretJson, format_atomic, parse_amount};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

/// Page indices are stable; `NAV_ORDER` is the order in the menu.
const SECTIONS: [&str; 11] = [
    "Overview",
    "Send",
    "Receive",
    "Transactions",
    "Mining",
    "My Full Node",
    "Network",
    "Settings",
    "Wallets",
    "Verify payment",
    "Privacy & Security",
];
const NODE_PAGE: usize = 5;
const WALLETS_PAGE: usize = 8;
const VERIFY_PAGE: usize = 9;
const SECURITY_PAGE: usize = 10;
const NAV_ORDER: [usize; 11] = [
    0,
    WALLETS_PAGE,
    1,
    2,
    3,
    VERIFY_PAGE,
    4,
    NODE_PAGE,
    6,
    SECURITY_PAGE,
    7,
];
/// The network this wallet runs. Mainnet since 0.4.0.
const NETWORK: Network = Network::Mainnet;
const GRAY: Color32 = Color32::from_gray(150);
const ERROR: Color32 = Color32::from_rgb(229, 115, 115);
const MIN_NEW_PASSWORD_CHARS: usize = 12;
const RECOVERY_WORDS: usize = 24;
const MINING_PAGE: usize = 4;
const AUTO_SCAN_INTERVAL: Duration = Duration::from_secs(5);
/// Pause before an automatically corrected miner restart.
const MINER_RETRY_DELAY: Duration = Duration::from_secs(3);
const APP_ICON_PNG: &[u8] = include_bytes!("../assets/genoryn-256.png");
/// Unlock is offered first only when there is a wallet file to unlock.
fn initial_wallet_mode(wallet_path: &str) -> usize {
    if PathBuf::from(wallet_path).is_file() {
        0
    } else {
        1
    }
}
#[derive(Clone)]
struct Settings {
    data_dir: String,
    wallet_path: String,
    rpc_url: String,
    bin_dir: String,
    /// GENORYN Lite: installed without genoryn-miner, so there is no Mining page.
    lite: bool,
    threads: u32,
    confirmation_target: u64,
    reference_price: String,
    node_config: String,
    /// Start an owned node when none answers at launch. Never starts mining.
    auto_node: bool,
    /// `--wallet` was given, so the remembered last wallet is not preselected.
    wallet_explicit: bool,
    smoke_test_frames: Option<u32>,
    /// Documentation screenshots into this folder, then exit.
    capture_dir: Option<String>,
    /// Read the unlock password from standard input, as the wallet CLI does.
    password_stdin: bool,
    /// Control positions for the native UI test (`layout_probe`).
    layout_report: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let directory = std::env::var_os("GENORYN_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".genoryn").join(NETWORK.data_dir_name()));
        Self {
            wallet_path: directory.join("wallet.gnr").display().to_string(),
            data_dir: directory.display().to_string(),
            rpc_url: format!("http://127.0.0.1:{}", NETWORK.base_port() + 1),
            bin_dir: String::new(),
            lite: false,
            threads: 1,
            confirmation_target: 10,
            reference_price: String::new(),
            node_config: String::new(),
            auto_node: true,
            wallet_explicit: false,
            smoke_test_frames: None,
            capture_dir: None,
            password_stdin: false,
            layout_report: None,
        }
    }
}
enum Event {
    NodeLog {
        directory: String,
        generation: u64,
        result: Result<String, String>,
    },
    ProofFile {
        generation: u64,
        result: Result<payment_proof::FileResult, String>,
    },
    Wallet {
        generation: u64,
        op: String,
        result: Result<SecretJson, String>,
    },
    Health {
        /// `network_generation` when the check began; older results are stale.
        generation: u64,
        endpoint: String,
        result: Box<Result<(HealthyNode, Value), String>>,
        /// `/v1/network` from a local node; `None` for remote or older nodes.
        privacy: Option<Value>,
    },
    Reconnect(Result<(), String>),
    Started {
        kind: &'static str,
        result: Result<OwnedChild, String>,
    },
    Stopped {
        kind: &'static str,
        result: Result<(), String>,
    },
    Mining(Value),
    Update(Result<Option<updates::Release>, String>),
    NameStatus {
        name: String,
        result: Result<Value, String>,
    },
    MyNames {
        owner: String,
        result: Result<Value, String>,
    },
    Security(Box<views_security::Diagnostics>),
    Preflight(Box<views_node::Preflight>),
}
#[derive(Clone)]
struct PaymentPreview {
    to: String,
    amount: String,
    fee: String,
    memo: String,
}
struct WalletApp {
    settings: Settings,
    page: usize,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    busy: Option<String>,
    health_busy: bool,
    process_busy: bool,
    /// The single network-mode change being applied to the owned node.
    network_transition: Option<network_transition::Transition>,
    network_outcome: Option<network_transition::Outcome>,
    /// Bumped on every mode change; health results from before it are dropped.
    network_generation: u64,
    auto_node_checked: bool,
    log_busy: bool,
    log_generation: u64,
    node_log: Option<String>,
    node_log_error: Option<String>,
    cancellation: Arc<AtomicBool>,
    wallet_worker: Option<thread::JoinHandle<()>>,
    /// The background scan lane, independent of `busy`.
    scan_busy: Option<String>,
    scan_cancellation: Arc<AtomicBool>,
    scan_worker: Option<thread::JoinHandle<()>>,
    process_worker: Option<thread::JoinHandle<()>>,
    status: String,
    network_error: Option<String>,
    node_info: Option<HealthyNode>,
    supply: Option<Value>,
    /// Network privacy status reported by the local node's admin RPC.
    network_privacy: Option<Value>,
    reconnect_busy: bool,
    last_health: Instant,
    last_input: Instant,
    password: Zeroizing<String>,
    confirm_password: Zeroizing<String>,
    seed_input: Zeroizing<String>,
    seed_backup: Zeroizing<String>,
    seed_ack: bool,
    wallet_mode: usize,
    unlocked: bool,
    generation: u64,
    address: String,
    balance: Option<String>,
    spendable: Option<String>,
    /// The last refused payment or registration, shown in its own card.
    action_error: Option<(String, String)>,
    started: Instant,
    capture: Option<capture::Capture>,
    /// Unlock once with the password read from standard input.
    auto_unlock: bool,
    page_since: (usize, Instant),
    wallet_height: Option<u64>,
    history: Vec<Value>,
    /// The rest of the last scan: mining rewards, outgoing payments, wallet
    /// mode, staleness and reserved amounts.
    sync_detail: Option<Value>,
    last_sync: Option<Instant>,
    last_auto_scan: Instant,
    auto_scan_running: bool,
    scan_started: Option<Instant>,
    last_scan_duration: Duration,
    last_submission: Option<String>,
    to: String,
    amount: String,
    fee: String,
    memo: String,
    preview: Option<PaymentPreview>,
    proofs: payment_proof::ProofUi,
    username: String,
    request_amount: String,
    payment_request: Option<String>,
    node: Option<OwnedChild>,
    miner: Option<OwnedChild>,
    mining_stats: Option<Value>,
    miner_attempt: miner_recovery::Attempt,
    miner_note: Option<String>,
    miner_retry_at: Option<Instant>,
    /// Address and wallet name the miner pays, fixed when mining starts.
    miner_destination: Option<(String, String)>,
    light: bool,
    clipboard_warning: Option<Instant>,
    update_status: updates::UpdateStatus,
    update_dismissed: bool,
    preferences: preferences::Preferences,
    pending_unlocks: std::collections::VecDeque<String>,
    pending_password: Option<Zeroizing<String>>,
    /// The batch being unlocked now.
    unlock_inflight: Vec<String>,
    /// Remembered wallets that did not open, by path key, with a short reason.
    unlock_failures: std::collections::HashMap<String, String>,
    /// Wallets unlocked so far and queued in total, while unlocking runs.
    unlock_progress: Option<(usize, usize)>,
    /// Changes on every lock; batch results from before a lock are discarded.
    lock_epoch: u64,
    scan_epoch: u64,
    last_batch_scan: Instant,
    batch_scan_duration: Duration,
    wallets_view: views_wallets::WalletsView,
    unlock_others: bool,
    sessions: Vec<sessions::WalletSession>,
    adding_wallet: bool,
    new_wallet_name: String,
    new_wallet_folder: String,
    wallet_files: Option<(String, Instant, Vec<String>)>,
    tx_view: views_wallet::HistoryView,
    names: names_ui::NamesUi,
    /// The Tor-only warning before opening the Explorer in a browser.
    explorer_confirm: bool,
    /// One-time notices about received, outgoing and mined payments.
    notifier: notifications::Notifier,
    security: views_security::SecurityUi,
    node_view: views_node::NodeView,
    recovery: recovery::RecoveryUi,
    /// Screen-capture protection while recovery words are on screen.
    capture_guard: Option<genoryn_screen_guard::Protection>,
}
impl WalletApp {
    fn new(cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        let mut app = Self::with_context(&cc.egui_ctx, settings);
        app.preferences = preferences::load(&app.settings.data_dir);
        network_privacy::set_active(
            network_privacy::Mode::parse(&app.preferences.network_mode)
                .unwrap_or(network_privacy::Mode::Direct),
        );
        app.light = app.preferences.light;
        set_style(&cc.egui_ctx, app.light);
        if !app.settings.wallet_explicit
            && let Some(last) = app
                .preferences
                .wallets
                .iter()
                .find(|path| PathBuf::from(path).is_file())
        {
            app.settings.wallet_path = last.clone();
            app.wallet_mode = 0;
            app.prepare_wallet_form(0);
        }
        if let Some(dir) = app.settings.capture_dir.clone() {
            app.capture = Some(capture::Capture::new(&dir));
        }
        if let Some(report) = &app.settings.layout_report {
            layout_probe::enable(PathBuf::from(report));
        }
        if app.settings.password_stdin {
            let mut line = Zeroizing::new(String::new());
            if std::io::stdin().read_line(&mut line).is_ok() {
                *app.password = line.trim_end_matches(['\r', '\n']).to_owned();
                app.auto_unlock = !app.password.is_empty();
            }
            line.zeroize();
        }
        if app.preferences.check_updates_at_launch
            && app.settings.smoke_test_frames.is_none()
            && app.capture.is_none()
        {
            app.check_for_updates();
        }
        app
    }
    fn with_context(ctx: &egui::Context, settings: Settings) -> Self {
        motion::install_fonts(ctx);
        set_style(ctx, false);
        let (tx, rx) = mpsc::channel();
        let wallet_mode = initial_wallet_mode(&settings.wallet_path);
        let mut app = Self {
            settings,
            page: 0,
            tx,
            rx,
            busy: None,
            health_busy: false,
            process_busy: false,
            network_transition: None,
            network_outcome: None,
            network_generation: 0,
            auto_node_checked: false,
            log_busy: false,
            log_generation: 0,
            node_log: None,
            node_log_error: None,
            cancellation: Arc::new(AtomicBool::new(false)),
            wallet_worker: None,
            scan_busy: None,
            scan_cancellation: Arc::new(AtomicBool::new(false)),
            scan_worker: None,
            process_worker: None,
            status: "Your keys stay on this computer.".into(),
            network_error: None,
            node_info: None,
            supply: None,
            network_privacy: None,
            reconnect_busy: false,
            last_health: Instant::now() - Duration::from_secs(10),
            last_input: Instant::now(),
            password: Zeroizing::new(String::new()),
            confirm_password: Zeroizing::new(String::new()),
            seed_input: Zeroizing::new(String::new()),
            seed_backup: Zeroizing::new(String::new()),
            seed_ack: false,
            wallet_mode,
            unlocked: false,
            generation: 0,
            address: String::new(),
            balance: None,
            spendable: None,
            action_error: None,
            started: Instant::now(),
            capture: None,
            auto_unlock: false,
            page_since: (0, Instant::now()),
            wallet_height: None,
            history: Vec::new(),
            sync_detail: None,
            last_sync: None,
            last_auto_scan: Instant::now(),
            auto_scan_running: false,
            scan_started: None,
            last_scan_duration: Duration::ZERO,
            last_submission: None,
            to: String::new(),
            amount: String::new(),
            fee: "0.00001".into(),
            memo: String::new(),
            preview: None,
            proofs: payment_proof::ProofUi::default(),
            username: String::new(),
            request_amount: String::new(),
            payment_request: None,
            node: None,
            miner: None,
            mining_stats: None,
            miner_attempt: miner_recovery::Attempt::fresh(1),
            miner_note: None,
            miner_retry_at: None,
            miner_destination: None,
            light: false,
            clipboard_warning: None,
            update_status: updates::UpdateStatus::NotChecked,
            update_dismissed: false,
            preferences: preferences::Preferences::default(),
            pending_unlocks: Default::default(),
            pending_password: None,
            unlock_inflight: Vec::new(),
            unlock_failures: Default::default(),
            unlock_progress: None,
            lock_epoch: 0,
            scan_epoch: 0,
            last_batch_scan: Instant::now(),
            batch_scan_duration: Duration::ZERO,
            wallets_view: Default::default(),
            unlock_others: true,
            sessions: Vec::new(),
            adding_wallet: false,
            new_wallet_name: String::new(),
            new_wallet_folder: String::new(),
            wallet_files: None,
            tx_view: views_wallet::HistoryView::default(),
            names: names_ui::NamesUi::default(),
            explorer_confirm: false,
            notifier: notifications::Notifier::default(),
            security: Default::default(),
            node_view: Default::default(),
            recovery: Default::default(),
            capture_guard: None,
        };
        app.prepare_wallet_form(wallet_mode);
        app
    }
}
/// Logo tile, wordmark and tagline. Its height equals the former three-line text
/// header, which keeps the menu entries where the native UI test expects them.
fn brand_header(ui: &mut egui::Ui, dark: bool, lite: bool) {
    let spacing = ui.spacing().item_spacing.y;
    let height = ui.fonts(|fonts| {
        fonts.row_height(&egui::FontId::monospace(28.0))
            + fonts.row_height(&egui::FontId::proportional(20.0))
            + fonts.row_height(&egui::FontId::proportional(8.8))
    }) + 2.0 * spacing
        + 8.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(162.0, height), egui::Sense::hover());
    let painter = ui.painter();
    let tile = 44.0;
    let logo = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 6.0), egui::vec2(tile, tile));
    icons::paint_logo(painter, logo, dark);
    let ink = if dark { Color32::WHITE } else { Color32::BLACK };
    let mut wordmark = egui::text::LayoutJob::default();
    wordmark.append(
        "GENORYN",
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(18.0, motion::semibold()),
            color: ink,
            extra_letter_spacing: 3.0,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(wordmark);
    painter.galley(
        egui::pos2(
            logo.right() + 12.0,
            logo.center().y - galley.size().y / 2.0 - 7.0,
        ),
        galley,
        ink,
    );
    painter.text(
        egui::pos2(logo.right() + 13.0, logo.center().y + 11.0),
        egui::Align2::LEFT_CENTER,
        if lite { "LITE · v" } else { "WALLET · v" }.to_owned() + updates::CURRENT,
        egui::FontId::monospace(9.5),
        GRAY,
    );
    painter.text(
        egui::pos2(rect.left() + 1.0, logo.bottom() + 18.0),
        egui::Align2::LEFT_CENTER,
        "PRIVATE MONETARY NETWORK",
        egui::FontId::proportional(8.8),
        GRAY,
    );
}
fn theme_toggle(ui: &mut egui::Ui, light: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
    let background = ui.visuals().panel_fill;
    let ink = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }
    icons::paint_theme(ui.painter(), rect.shrink(3.0), light, ink, background);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Appearance"));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
impl WalletApp {
    fn set_appearance(&mut self, ctx: &egui::Context, light: bool) {
        self.light = light;
        set_style(ctx, light);
        self.preferences.light = light;
        self.save_preferences();
    }
}
/// An amount with the GNR sign before it and the unit after it.
/// The exact amount with thousands grouped: `54,000.9999`. Never rounded.
/// A count with thousands grouped: `10,000`.
fn grouped_count(count: usize) -> String {
    grouped_amount(&(count as u128 * 1_000_000_000_000).to_string())
}
fn grouped_amount(atomic: &str) -> String {
    let exact = format_atomic(atomic);
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    if !whole.bytes().all(|b| b.is_ascii_digit()) {
        return exact;
    }
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if fraction.is_empty() {
        grouped
    } else {
        format!("{grouped}.{fraction}")
    }
}
#[cfg(test)]
mod amount_tests {
    #[test]
    fn amounts_are_exact_and_grouped_by_thousands() {
        assert_eq!(super::grouped_amount("999000000000000"), "999");
        assert_eq!(super::grouped_amount("54000999900000000"), "54,000.9999");
        assert_eq!(super::grouped_amount("7000999950000000"), "7,000.99995");
        assert_eq!(super::grouped_amount("21000000000000000000"), "21,000,000");
        assert_eq!(super::grouped_amount("1"), "0.000000000001");
        assert_eq!(super::grouped_count(10_000), "10,000");
        assert_eq!(super::grouped_count(0), "0");
        assert_eq!(super::grouped_amount("x"), "Unavailable");
    }
}
/// The whole amount always shows: a figure wider than the space left shrinks
/// instead of being cut off.
fn gnr_amount(ui: &mut egui::Ui, atomic: Option<&str>, size: f32, color: Option<Color32>) {
    let ink = color.unwrap_or_else(|| ui.visuals().text_color());
    let text = atomic.map(grouped_amount).unwrap_or_else(|| "—".into());
    let font = |size: f32| {
        if size >= 24.0 {
            egui::FontId::new(size, motion::display())
        } else {
            egui::FontId::proportional(size)
        }
    };
    let measure = |ui: &egui::Ui, size: f32| {
        let figure = ui
            .fonts(|fonts| fonts.layout_no_wrap(text.clone(), font(size), ink))
            .size()
            .x;
        // Sign, unit and spacing scale with the figure.
        figure + size * (0.82 + 0.45 * 1.7 + 0.44)
    };
    let available = ui.available_width();
    let mut size = size;
    while size > 12.0 && measure(ui, size) > available {
        size = (size * 0.92).max(12.0);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size * 0.22;
        let (sign, _) = ui.allocate_exact_size(egui::vec2(size * 0.82, size), egui::Sense::hover());
        icons::paint_sign(ui.painter(), sign, ink);
        let figure = RichText::new(&text).size(size).color(ink);
        ui.add(
            egui::Label::new(if size >= 24.0 {
                figure
                    .family(motion::display())
                    .extra_letter_spacing(-0.04 * size)
            } else {
                figure
            })
            .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.label(
            RichText::new("GNR")
                .size((size * 0.45).max(11.0))
                .color(GRAY),
        );
    });
}
/// One sidebar entry; the 162x42 footprint is relied on by the native UI test.
fn nav_item(
    ui: &mut egui::Ui,
    icon: icons::Icon,
    label: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(162.0, 42.0), sense);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label)
    });
    let dark = ui.visuals().dark_mode;
    let hovered = enabled && response.hovered();
    if !enabled {
        icons::paint(
            ui.painter(),
            icons::square(rect.left() + 24.0, rect.center().y, 17.0),
            icon,
            Color32::from_gray(if dark { 70 } else { 175 }),
        );
        ui.painter().text(
            egui::pos2(rect.left() + 46.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(15.0),
            Color32::from_gray(if dark { 70 } else { 175 }),
        );
        return response.on_hover_text("Unlock a wallet first");
    }
    let painter = ui.painter();
    let fill = if selected {
        Color32::from_gray(if dark { 30 } else { 222 })
    } else if hovered {
        Color32::from_gray(if dark { 19 } else { 233 })
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 7.0, fill);
    let strong = if dark { Color32::WHITE } else { Color32::BLACK };
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(
                rect.min + egui::vec2(0.0, 11.0),
                egui::vec2(3.0, rect.height() - 22.0),
            ),
            1.5,
            strong,
        );
    }
    let ink = if selected || hovered {
        strong
    } else {
        Color32::from_gray(if dark { 165 } else { 85 })
    };
    icons::paint(
        painter,
        icons::square(rect.left() + 24.0, rect.center().y, 17.0),
        icon,
        ink,
    );
    painter.text(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(15.0),
        ink,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for WalletApp {
    /// Seams between panels show the clear color; match the page background so no
    /// dark line appears in light mode.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let gray = if self.light { 246 } else { 7 };
        egui::Rgba::from(Color32::from_gray(gray)).to_array()
    }
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.poll();
        if ctx.input(|i| !i.events.is_empty()) {
            self.last_input = Instant::now();
        }
        if (self.unlocked || !self.seed_backup.is_empty())
            && self.last_input.elapsed() > Duration::from_secs(300)
        {
            self.lock();
        }
        if self.last_health.elapsed() > Duration::from_secs(5) {
            self.refresh_network();
        }
        if self.unlocked {
            self.adding_wallet = false;
            self.process_pending_unlocks();
            self.names_tick();
        } else {
            // Nothing but the unlock screen is reachable without a password.
            self.page = 0;
        }
        self.auto_scan();
        self.network_transition_tick();
        self.miner_retry_tick();
        if self
            .clipboard_warning
            .is_some_and(|t| t.elapsed() > Duration::from_secs(30))
        {
            self.clipboard_warning = None;
        }
        egui::SidePanel::left("navigation")
            .exact_width(206.0)
            .frame(
                egui::Frame::new()
                    .fill(if self.light {
                        Color32::from_gray(240)
                    } else {
                        Color32::from_gray(5)
                    })
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_gray(if self.light { 226 } else { 24 }),
                    ))
                    .inner_margin(22.0),
            )
            .show(ctx, |ui| {
                ui.add_space(14.0);
                brand_header(ui, !self.light, self.settings.lite);
                ui.add_space(20.0);
                let unlocked = self.unlocked;
                // Entries 42 points apart; a short window scrolls them and keeps
                // the lock button below.
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical()
                    .id_salt("navigation_entries")
                    .max_height((ui.available_height() - 110.0).max(42.0 * 3.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for index in NAV_ORDER {
                            if self.settings.lite && index == MINING_PAGE {
                                continue;
                            }
                            let response = nav_item(
                                ui,
                                icons::SECTION_ICONS[index],
                                SECTIONS[index],
                                self.page == index,
                                unlocked,
                            );
                            layout_probe::record(
                                &format!("nav:{}", SECTIONS[index]),
                                response.rect,
                            );
                            if response.clicked() {
                                self.page = index;
                            }
                        }
                    });
                ui.spacing_mut().item_spacing.y = 12.0;
                // A short window keeps the menu and the lock button; the link waits.
                if ui.available_height() > 190.0 {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);
                    self.explorer_link(ui);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("EXPERIMENTAL · NOT AUDITED")
                            .size(10.0)
                            .color(GRAY),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(NETWORK.name().to_uppercase())
                            .monospace()
                            .size(12.0),
                    );
                    ui.add_space(12.0);
                    if self.unlocked
                        && !self.password.is_empty()
                        && ui.button("Lock wallet").clicked()
                    {
                        self.lock();
                    }
                });
            });
        egui::TopBottomPanel::top("topbar")
            .exact_height(64.0)
            .frame(
                egui::Frame::new()
                    .fill(if self.light {
                        Color32::from_gray(246)
                    } else {
                        Color32::from_gray(7)
                    })
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_gray(if self.light { 228 } else { 26 }),
                    ))
                    .inner_margin(egui::Margin::symmetric(40, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    self.wallet_switcher(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme_toggle(ui, self.light)
                            .on_hover_text(if self.light {
                                "Switch to dark"
                            } else {
                                "Switch to light"
                            })
                            .clicked()
                        {
                            self.set_appearance(ui.ctx(), !self.light);
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(match self.open_wallet_count() {
                                0 => "Wallet locked".to_owned(),
                                1 => "Wallet unlocked".to_owned(),
                                count => format!("{count} wallets unlocked"),
                            })
                            .small(),
                        );
                        if self.open_wallet_count() > 1 {
                            ui.separator();
                            let (total, _) = self.combined_balance();
                            ui.horizontal(|ui| {
                                gnr_amount(ui, Some(&total.to_string()), 13.0, None);
                                ui.label(RichText::new("Total").small().color(GRAY));
                            });
                        }
                        ui.separator();
                        let transport = self
                            .network_privacy
                            .as_ref()
                            .and_then(|status| status["mode"].as_str())
                            .and_then(network_privacy::Mode::parse)
                            .map(|mode| format!(" · {}", mode.label()))
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(
                                self.node_info
                                    .as_ref()
                                    .map(|n| {
                                        format!("Mainnet · Block {}{transport}", n.info.height)
                                    })
                                    .unwrap_or_else(|| "Node not connected".into()),
                            )
                            .small()
                            .color(GRAY),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("status")
            .min_height(45.0)
            .frame(
                egui::Frame::new()
                    .fill(if self.light {
                        Color32::from_gray(242)
                    } else {
                        Color32::from_gray(8)
                    })
                    .inner_margin(egui::Margin::symmetric(28, 12)),
            )
            .show(ctx, |ui| {
                // The scan indicator takes its place on the right first; the
                // status uses the rest and is cut short rather than overlapping.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(label) = &self.scan_busy {
                        let target = if controller::is_batch_label(label) {
                            "other wallets".to_owned()
                        } else {
                            label
                                .strip_prefix("sync@")
                                .map(sessions::wallet_name)
                                .unwrap_or_else(|| {
                                    sessions::wallet_name(&self.settings.wallet_path)
                                })
                        };
                        ui.label(
                            RichText::new(format!("Scanning {target} in the background"))
                                .small()
                                .color(GRAY),
                        );
                        motion::loader(ui, 13.0);
                        ui.add_space(12.0);
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if let Some(op) = &self.busy {
                            motion::loader(ui, 16.0);
                            ui.label(format!("{op} in progress…"));
                        } else if self.process_busy {
                            motion::loader(ui, 16.0);
                            ui.label("Process operation in progress…");
                        } else {
                            ui.add(
                                egui::Label::new(RichText::new(&self.status).small().color(GRAY))
                                    .truncate(),
                            )
                            .on_hover_text(&self.status);
                        }
                        if self.clipboard_warning.is_some() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        "Copied. Other applications may read your clipboard.",
                                    )
                                    .small(),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(if self.light {
                        Color32::from_gray(246)
                    } else {
                        Color32::from_gray(7)
                    })
                    .inner_margin(egui::Margin::symmetric(40, 34)),
            )
            .show(ctx, |ui| {
                if self.page_since.0 != self.page {
                    self.page_since = (self.page, Instant::now());
                }
                let shown = motion::page_fade(ctx, self.page_since.1);
                egui::ScrollArea::vertical()
                    .id_salt(("page", self.page))
                    .show(ui, |ui| {
                        ui.set_opacity(shown);
                        ui.add_space(10.0 * (1.0 - shown));
                        // Overview and Transactions use wide windows; forms stay readable.
                        let limit = match self.page {
                            0 => 1600.0,
                            3 | WALLETS_PAGE => 1280.0,
                            _ => 1040.0,
                        };
                        ui.set_max_width(ui.available_width().min(limit));
                        layout_probe::page(if self.unlocked {
                            SECTIONS[self.page]
                        } else {
                            "Locked"
                        });
                        match self.page {
                            0 => self.overview(ui),
                            WALLETS_PAGE => self.wallets_page(ui),
                            VERIFY_PAGE => self.verify_page(ui),
                            1 => self.send(ui),
                            2 => self.receive(ui),
                            3 => self.transactions(ui),
                            MINING_PAGE if !self.settings.lite => self.mining(ui),
                            NODE_PAGE => self.node_page(ui),
                            6 => self.network(ui),
                            SECURITY_PAGE => self.security_page(ui),
                            _ => self.settings_page(ui),
                        }
                    });
            });
        self.proof_export_window(ctx);
        self.update_notice(ctx);
        self.node_log_window(ctx);
        self.recovery_windows(ctx);
        // Recovery words on screen (a new wallet's, revealed ones, or ones being
        // typed to restore) keep the window out of screen captures.
        let secret_on_screen = !self.seed_backup.is_empty()
            || self.recovery.words_on_screen()
            || (!self.unlocked && self.wallet_mode == 2 && !self.seed_input.is_empty());
        if secret_on_screen != self.capture_guard.is_some() {
            self.capture_guard = if secret_on_screen {
                Some(genoryn_screen_guard::protect(frame))
            } else {
                genoryn_screen_guard::release(frame);
                None
            };
        }
        self.name_confirm_window(ctx);
        self.explorer_confirm_window(ctx);
        self.toasts_overlay(ctx);
        if let Some(preview) = self.preview.clone() {
            egui::Window::new("Confirm private payment").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,[0.0,0.0]).default_width(480.0).show(ctx,|ui|{
                ui.label("Review carefully. A confirmed blockchain payment cannot be reversed.");ui.add_space(14.0);ui.label(format!("Recipient: {}",preview.to));ui.label(format!("Amount: {} GNR",format_atomic(&preview.amount)));ui.label(format!("Fee: {} GNR",format_atomic(&preview.fee)));ui.label("Network: GENORYN mainnet");ui.add_space(16.0);ui.horizontal(|ui|{
                    let total=preview.amount.parse::<u64>().ok().zip(preview.fee.parse::<u64>().ok()).and_then(|(a,f)|a.checked_add(f));let short=total.and_then(|t|self.shortfall(t));if let Some(reason)=&short{ui.label(RichText::new(reason.as_str()).color(ERROR));}
                    if ui.add_enabled(self.busy.is_none()&&short.is_none(),primary(ui, "Confirm and send")).clicked(){self.preview=None;self.action_error=None;self.request("send",json!({"to":preview.to,"amount_atomic":preview.amount,"fee_atomic":preview.fee,"memo":preview.memo}));}
                    if ui.button("Cancel").clicked(){self.preview=None;}
                });
            });
        }
        motion::splash(ctx, self.started, self.light);
        if self.auto_unlock && !self.unlocked && self.busy.is_none() {
            self.auto_unlock = false;
            self.wallet_mode = 0;
            self.request("open", json!({}));
        }
        self.capture_tick(ctx);
        layout_probe::end_frame(ctx.pixels_per_point());
        if let Some(frames) = &mut self.settings.smoke_test_frames {
            *frames = frames.saturating_sub(1);
            if *frames == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
impl Drop for WalletApp {
    fn drop(&mut self) {
        self.lock();
        if let Some(worker) = self.wallet_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.scan_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.process_worker.take() {
            let _ = worker.join();
        }
        while let Ok(event) = self.rx.try_recv() {
            drop(event);
        }
        if let Some(child) = self.miner.take() {
            let _ = protocol::stop_owned_child(child);
        }
        if let Some(child) = self.node.take() {
            if self.preferences.node_keep_running {
                // The user agreed: the node keeps validating after the window closes.
                views_node::leave_running(&self.settings.data_dir, child);
            } else {
                let _ = protocol::stop_owned_child(child);
            }
        }
    }
}
fn set_style(ctx: &egui::Context, light: bool) {
    // Bind the selected appearance before changing its style. Otherwise a later
    // Windows system-theme event can switch to egui's uncustomized other style.
    ctx.set_theme(if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    });
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.override_text_color = Some(Color32::from_gray(if light { 30 } else { 224 }));
    visuals.selection.bg_fill = Color32::from_gray(if light { 205 } else { 55 });
    visuals.selection.stroke = Stroke::new(
        1.0,
        if light {
            Color32::BLACK
        } else {
            Color32::WHITE
        },
    );
    visuals.text_cursor.stroke = Stroke::new(
        2.0,
        if light {
            Color32::BLACK
        } else {
            Color32::WHITE
        },
    );
    visuals.hyperlink_color = if light {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    visuals.warn_fg_color = Color32::from_gray(170);
    visuals.error_fg_color = if light {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let gray = |light_value: u8, dark_value: u8| {
        Color32::from_gray(if light { light_value } else { dark_value })
    };
    let radius = egui::CornerRadius::same(10);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = gray(255, 12);
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, gray(220, 38));
    widgets.inactive.bg_fill = gray(238, 24);
    widgets.inactive.weak_bg_fill = gray(238, 24);
    widgets.inactive.bg_stroke = Stroke::new(1.0, gray(214, 48));
    widgets.hovered.bg_fill = gray(228, 36);
    widgets.hovered.weak_bg_fill = gray(228, 36);
    widgets.hovered.bg_stroke = Stroke::new(1.0, gray(150, 105));
    widgets.active.bg_fill = gray(208, 58);
    widgets.active.weak_bg_fill = gray(208, 58);
    widgets.active.bg_stroke = Stroke::new(1.0, gray(40, 220));
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    visuals.extreme_bg_color = gray(250, 17);
    visuals.faint_bg_color = gray(244, 22);
    visuals.window_fill = gray(255, 12);
    visuals.window_stroke = Stroke::new(1.0, gray(220, 44));
    visuals.window_corner_radius = egui::CornerRadius::same(14);
    visuals.menu_corner_radius = radius;
    visuals.window_shadow = egui::Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(if light { 40 } else { 120 }),
    };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if light { 30 } else { 100 }),
    };
    ctx.set_visuals(visuals);
    let mut style = (*ctx.style()).clone();
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(24.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(14.0, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(12.0, 12.0);
    style.spacing.button_padding = egui::vec2(18.0, 10.0);
    style.spacing.interact_size = egui::vec2(40.0, 40.0);
    style.spacing.text_edit_width = 480.0;
    style.spacing.combo_width = 220.0;
    style.spacing.window_margin = egui::Margin::same(24);
    style.spacing.menu_margin = egui::Margin::same(10);
    ctx.set_style(style);
}
/// A single-line input with room around its text; every form uses it.
fn field(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(14, 11))
}
fn area(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text).margin(egui::Margin::symmetric(14, 11))
}
impl WalletApp {
    /// The wallet the running miner pays, when it is not the wallet open now.
    fn mining_elsewhere(&self) -> Option<&str> {
        let (address, name) = self.miner_destination.as_ref()?;
        (self.miner.is_some() || self.miner_retry_at.is_some())
            .then_some(())
            .filter(|()| *address != self.address)
            .map(|()| name.as_str())
    }
    /// `Some(reason)` when the active wallet's matured balance cannot cover `needed`.
    fn shortfall(&self, needed: u64) -> Option<String> {
        let spendable = self.spendable.as_deref()?.parse::<u64>().ok()?;
        (needed > spendable).then(|| {
            format!(
                "Not enough spendable GNR: {} GNR needed including the fee, {} GNR spendable. Mined coins become spendable 60 blocks after they are found.",
                format_atomic(&needed.to_string()),
                format_atomic(&spendable.to_string())
            )
        })
    }
    fn action_error_for(&self, op: &str) -> Option<&str> {
        self.action_error
            .as_ref()
            .filter(|(failed, _)| failed == op)
            .map(|(_, message)| message.as_str())
    }
}
/// The one main action of a form: filled with the strongest ink.
fn primary(ui: &egui::Ui, label: &str) -> egui::Button<'static> {
    let (fill, ink) = if ui.visuals().dark_mode {
        (Color32::from_gray(238), Color32::from_gray(12))
    } else {
        (Color32::from_gray(18), Color32::WHITE)
    };
    egui::Button::new(
        RichText::new(label.to_owned())
            .family(motion::semibold())
            .color(ink),
    )
    .fill(fill)
    .stroke(Stroke::NONE)
    .corner_radius(22.0)
}
fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(
        RichText::new(title)
            .family(motion::display())
            .size(28.0)
            .extra_letter_spacing(-0.6)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(RichText::new(subtitle).size(15.0).color(GRAY));
}
/// Section labels in the site's style: spaced monospace capitals.
fn small_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        RichText::new(title.to_uppercase())
            .monospace()
            .size(11.5)
            .extra_letter_spacing(1.4)
            .color(GRAY),
    );
    ui.add_space(8.0);
}
fn diagnostic(ui: &mut egui::Ui, label: &str, value: impl std::fmt::Display) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [210.0, 20.0],
            egui::Label::new(RichText::new(label).color(GRAY)),
        );
        ui.label(value.to_string());
    });
}
fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().widgets.noninteractive.bg_fill)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(14.0)
        .inner_margin(egui::Margin::symmetric(28, 26))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}
fn draw_qr(ui: &mut egui::Ui, address: &str, size: f32) {
    let Ok(qr) = qrcode::QrCode::new(address.as_bytes()) else {
        ui.label("Receiving address cannot be encoded as QR.");
        return;
    };
    let width = qr.width();
    let quiet = 4;
    let scale = size / (width + 2 * quiet) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
    for y in 0..width {
        for x in 0..width {
            if qr[(x, y)] == qrcode::Color::Dark {
                let min =
                    rect.min + egui::vec2((x + quiet) as f32 * scale, (y + quiet) as f32 * scale);
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(min, egui::vec2(scale + 0.1, scale + 0.1)),
                    0.0,
                    Color32::BLACK,
                );
            }
        }
    }
}
fn parse_settings(args: impl IntoIterator<Item = String>) -> Result<Option<Settings>> {
    let mut settings = Settings::default();
    let mut args = args.into_iter();
    let mut wallet_override = None;
    while let Some(arg) = args.next() {
        if arg == "--help" {
            return Ok(None);
        }
        if arg == "--password-stdin" {
            settings.password_stdin = true;
            continue;
        }
        ensure!(
            matches!(
                arg.as_str(),
                "--data-dir"
                    | "--wallet"
                    | "--rpc-url"
                    | "--bin-dir"
                    | "--node-config"
                    | "--auto-node"
                    | "--smoke-test-frames"
                    | "--capture-pages"
                    | "--layout-report"
            ),
            "Unknown wallet argument. Use --help."
        );
        let value = args
            .next()
            .context("Wallet option requires a value. Use --help.")?;
        ensure!(
            !value.is_empty() && !value.starts_with("--"),
            "Wallet option requires a nonempty value. Use --help."
        );
        match arg.as_str() {
            "--data-dir" => {
                settings.data_dir = value;
            }
            "--wallet" => {
                wallet_override = Some(value);
            }
            "--rpc-url" => {
                settings.rpc_url = value;
            }
            "--bin-dir" => {
                settings.bin_dir = value;
            }
            "--node-config" => {
                settings.node_config = value;
            }
            "--auto-node" => {
                settings.auto_node = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => anyhow::bail!("--auto-node requires on or off"),
                };
            }
            "--smoke-test-frames" => {
                settings.smoke_test_frames = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|value| (2..=1000).contains(value))
                        .context("--smoke-test-frames requires an integer from 2 to 1000")?,
                );
            }
            "--capture-pages" => {
                settings.capture_dir = Some(value);
            }
            "--layout-report" => {
                settings.layout_report = Some(value);
            }
            _ => unreachable!("options are checked before consuming a value"),
        }
    }
    settings.wallet_explicit = wallet_override.is_some();
    settings.wallet_path = wallet_override.unwrap_or_else(|| {
        PathBuf::from(&settings.data_dir)
            .join("wallet.gnr")
            .display()
            .to_string()
    });
    Ok(Some(settings))
}
fn main() -> eframe::Result<()> {
    let settings = match parse_settings(std::env::args().skip(1)) {
        Ok(Some(mut settings)) => {
            settings.lite = protocol::sibling_binary("genoryn-miner", &settings.bin_dir).is_err();
            settings
        }
        Ok(None) => {
            println!(
                "GENORYN wallet (mainnet)\n--data-dir PATH --wallet PATH --rpc-url URL --bin-dir PATH --node-config PATH --auto-node on|off --smoke-test-frames N\nDocumentation screenshots: --capture-pages DIR --password-stdin\nUI test control positions: --layout-report FILE\nNever pass a seed or password on the command line."
            );
            return Ok(());
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let title = if settings.lite {
        "GENORYN Lite"
    } else {
        "GENORYN Wallet"
    };
    let options = eframe::NativeOptions {
        #[cfg(target_os = "windows")]
        renderer: eframe::Renderer::Wgpu,
        #[cfg(not(target_os = "windows"))]
        renderer: eframe::Renderer::Glow,
        #[cfg(target_os = "windows")]
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetupCreateNew {
                instance_descriptor: wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::DX12,
                    ..Default::default()
                },
                ..Default::default()
            }
            .into(),
            ..Default::default()
        },
        viewport: {
            let viewport = egui::ViewportBuilder::default()
                .with_inner_size([1180.0, 820.0])
                .with_min_inner_size([900.0, 680.0])
                .with_title(title);
            match eframe::icon_data::from_png_bytes(APP_ICON_PNG) {
                Ok(icon) => viewport.with_icon(icon),
                Err(_) => viewport,
            }
        },
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| Ok(Box::new(WalletApp::new(cc, settings)))),
    )
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    #[test]
    fn system_theme_changes_preserve_selected_monochrome_style_and_layout() {
        for light in [false, true] {
            let ctx = egui::Context::default();
            set_style(&ctx, light);
            let selected = if light {
                egui::Theme::Light
            } else {
                egui::Theme::Dark
            };
            let opposite = if light {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            };
            let _ = ctx.run(
                egui::RawInput {
                    system_theme: Some(opposite),
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(ctx.theme(), selected);
                    let style = ctx.style();
                    assert_eq!(style.spacing.item_spacing, egui::vec2(12.0, 12.0));
                    assert_eq!(style.spacing.button_padding, egui::vec2(18.0, 10.0));
                    assert_eq!(
                        style.visuals.override_text_color,
                        Some(Color32::from_gray(if light { 30 } else { 224 }))
                    );
                    let selection = style.visuals.selection.bg_fill;
                    assert_eq!(selection.r(), selection.g());
                    assert_eq!(selection.g(), selection.b());
                },
            );
        }
    }
    #[test]
    fn explicit_wallet_path_is_independent_of_option_order() {
        for args in [
            ["--wallet", "custom.wallet", "--data-dir", "node-data"],
            ["--data-dir", "node-data", "--wallet", "custom.wallet"],
        ] {
            let settings = parse_settings(args.map(str::to_owned)).unwrap().unwrap();
            assert_eq!(settings.wallet_path, "custom.wallet");
            assert_eq!(settings.data_dir, "node-data");
        }
        let settings = parse_settings(["--data-dir", "node-data"].map(str::to_owned))
            .unwrap()
            .unwrap();
        assert_eq!(
            settings.wallet_path,
            PathBuf::from("node-data")
                .join("wallet.gnr")
                .display()
                .to_string()
        );
    }
    #[test]
    fn malformed_options_fail_before_opening_a_window() {
        for args in [
            vec!["--wallet"],
            vec!["--wallet", "--data-dir", "other"],
            vec!["--data-dir", ""],
            vec!["--unknown", "value"],
            vec!["--smoke-test-frames", "1"],
            vec!["--smoke-test-frames", "1001"],
            vec!["--smoke-test-frames", "not-a-number"],
        ] {
            assert!(parse_settings(args.into_iter().map(str::to_owned)).is_err());
        }
        assert!(parse_settings(["--help".to_owned()]).unwrap().is_none());
        assert_eq!(
            parse_settings(["--smoke-test-frames", "5"].map(str::to_owned))
                .unwrap()
                .unwrap()
                .smoke_test_frames,
            Some(5)
        );
    }
    #[test]
    fn lock_clears_secrets_and_invalidates_late_results() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        *app.password = "test-only password".into();
        *app.seed_backup = "test-only non-mnemonic".into();
        app.unlocked = true;
        app.balance = Some("100".into());
        app.amount = "123.45".into();
        app.request_amount = "6.7".into();
        let generation = app.generation;
        app.lock();
        assert!(app.password.is_empty());
        assert!(app.seed_backup.is_empty());
        assert!(!app.unlocked);
        assert!(app.balance.is_none());
        assert!(app.amount.is_empty());
        assert!(app.request_amount.is_empty());
        assert_ne!(app.generation, generation);
        assert!(app.cancellation.load(Ordering::Acquire));
        app.tx
            .send(Event::Wallet {
                generation,
                op: "open".into(),
                result: Ok(SecretJson(json!({"address":"must-not-unlock"}))),
            })
            .unwrap();
        app.poll();
        assert!(!app.unlocked);
        assert!(app.address.is_empty());
    }
    #[test]
    fn every_screen_renders_monochrome_with_honest_empty_state() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        for light in [false, true] {
            app.light = light;
            set_style(&ctx, light);
            for page in 0..SECTIONS.len() {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1180.0, 820.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| match page {
                            0 => app.overview(ui),
                            1 => app.send(ui),
                            2 => app.receive(ui),
                            3 => app.transactions(ui),
                            4 => app.mining(ui),
                            NODE_PAGE => app.node_page(ui),
                            6 => app.network(ui),
                            WALLETS_PAGE => app.wallets_page(ui),
                            VERIFY_PAGE => app.verify_page(ui),
                            SECURITY_PAGE => app.security_page(ui),
                            _ => app.settings_page(ui),
                        });
                    },
                );
                let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                for primitive in primitives {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        for vertex in mesh.vertices {
                            let color = vertex.color;
                            assert_eq!(color.r(), color.g(), "non-monochrome page {page}");
                            assert_eq!(color.g(), color.b(), "non-monochrome page {page}");
                        }
                    }
                }
                assert!(app.balance.is_none());
                assert!(app.node_info.is_none());
                assert!(app.miner.is_none());
            }
        }
    }
}
