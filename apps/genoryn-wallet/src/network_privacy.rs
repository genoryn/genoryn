//! Network privacy in the wallet: the mode the owned node starts with, the Tor
//! status that node reports, and the rules for the wallet's own traffic.
//!
//! Network privacy (Tor) hides this computer's IP address from peers. It is
//! separate from on-chain privacy: shielded notes hide amounts and addresses in
//! every mode. No mode is presented as making anyone completely anonymous.
use super::*;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU8, Ordering},
};

pub(super) const DEFAULT_SOCKS: &str = "127.0.0.1:9050";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Direct,
    Hybrid,
    TorOnly,
}
impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Direct, Mode::Hybrid, Mode::TorOnly];
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "direct" => Some(Self::Direct),
            "hybrid" => Some(Self::Hybrid),
            "tor-only" => Some(Self::TorOnly),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Hybrid => "hybrid",
            Self::TorOnly => "tor-only",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Direct => "Direct",
            Self::Hybrid => "Hybrid",
            Self::TorOnly => "Tor only",
        }
    }
    pub fn summary(self) -> &'static str {
        match self {
            Self::Direct => "Standard peer-to-peer connections, as in earlier releases.",
            Self::Hybrid => {
                "Clearnet peers directly; onion peers and your payments through Tor when Tor peers are available."
            }
            Self::TorOnly => {
                "Every peer connection, peer lookup and payment broadcast goes through Tor. Nothing falls back to a direct connection: if Tor stops, networking pauses."
            }
        }
    }
    /// Shown prominently whenever the mode exposes this computer's IP address.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::Direct => Some(
                "Privacy warning: every peer sees this computer's public IP address and may link it to payments this node broadcasts first.",
            ),
            Self::Hybrid => Some(
                "Privacy warning: direct clearnet connections reveal this computer's public IP address to those peers.",
            ),
            Self::TorOnly => None,
        }
    }
    fn index(self) -> u8 {
        match self {
            Self::Direct => 0,
            Self::Hybrid => 1,
            Self::TorOnly => 2,
        }
    }
}
/// Which Tor the node uses in hybrid and tor-only modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Engine {
    /// The Tor client inside the node: nothing to install or keep running.
    BuiltIn,
    /// A Tor daemon or Tor Browser the user runs, reached at its SOCKS port.
    External,
}
impl Engine {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "built-in" => Some(Self::BuiltIn),
            "external" => Some(Self::External),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::External => "external",
        }
    }
    fn of(preferences: &preferences::Preferences) -> Self {
        Self::parse(&preferences.tor_engine).unwrap_or(Self::BuiltIn)
    }
}
pub(super) const LIGHT_TRUST: &str = "Syncs and payments go only through Tor to these nodes. They never see your addresses or balance, and every block is fully validated on this computer. They can see when you sync and could withhold new blocks, so use two or more independent nodes. Usernames, names and mining need a node of your own.";
pub(super) const LAYERS: &str = "Network privacy hides your IP address from peers. On-chain privacy (shielded notes) hides amounts and addresses in every mode. Neither makes anyone 100% anonymous: a compromised computer, timing analysis or an observer of both ends of a Tor circuit can still reveal information.";

/// The active mode for the wallet's own traffic, readable from worker threads.
static ACTIVE: AtomicU8 = AtomicU8::new(0);
pub(super) fn set_active(mode: Mode) {
    ACTIVE.store(mode.index(), Ordering::Release);
}
pub(super) fn active() -> Mode {
    match ACTIVE.load(Ordering::Acquire) {
        1 => Mode::Hybrid,
        2 => Mode::TorOnly,
        _ => Mode::Direct,
    }
}
pub(super) fn is_loopback_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| {
        url.host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|ip| ip.is_loopback())
    })
}
/// Wallet RPC goes to the configured endpoint. In tor-only mode only the user's
/// own loopback node is allowed: a remote endpoint would see this computer's IP.
pub(super) fn guard_rpc(url: &str) -> Result<()> {
    ensure!(
        active() != Mode::TorOnly || is_loopback_url(url),
        "Tor-only mode: the wallet talks only to your own local node. A remote RPC endpoint would bypass Tor, so nothing was sent."
    );
    Ok(())
}
pub(super) fn parse_socket(text: &str, what: &str) -> Result<SocketAddr> {
    text.trim().parse::<SocketAddr>().map_err(|_| {
        anyhow::anyhow!("{what} must be an IP address and port, for example {DEFAULT_SOCKS}")
    })
}
/// How auxiliary HTTPS such as update checks leaves this computer. Only
/// tor-only mode constrains it, and then only through Tor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Auxiliary {
    Direct,
    /// External Tor's SOCKS port.
    Socks(SocketAddr),
    /// The built-in client's loopback SOCKS bridge, which the local node hands
    /// out with per-process credentials to holders of its admin token.
    BuiltIn {
        rpc_url: String,
        token_file: PathBuf,
    },
}
pub(super) fn auxiliary_route(
    preferences: &preferences::Preferences,
    settings: &Settings,
) -> Result<Auxiliary> {
    if Mode::parse(&preferences.network_mode) != Some(Mode::TorOnly) {
        return Ok(Auxiliary::Direct);
    }
    Ok(match Engine::of(preferences) {
        Engine::External => {
            Auxiliary::Socks(parse_socket(&preferences.tor_socks, "Tor SOCKS proxy")?)
        }
        Engine::BuiltIn => {
            ensure!(
                is_loopback_url(&settings.rpc_url),
                "Tor-only mode: the built-in Tor is reached through your own local node"
            );
            Auxiliary::BuiltIn {
                rpc_url: settings.rpc_url.clone(),
                token_file: PathBuf::from(&settings.data_dir).join("admin.token"),
            }
        }
    })
}
/// How long a background request waits for the node's built-in Tor, which
/// usually becomes ready within seconds of the wallet starting the node.
const BUILT_IN_PATIENCE: Duration = Duration::from_secs(180);
/// The proxy URL for `route`; never falls back to a direct connection.
pub(super) fn auxiliary_proxy(route: &Auxiliary) -> Result<Option<Zeroizing<String>>> {
    auxiliary_proxy_within(route, BUILT_IN_PATIENCE)
}
fn auxiliary_proxy_within(
    route: &Auxiliary,
    patience: Duration,
) -> Result<Option<Zeroizing<String>>> {
    Ok(match route {
        Auxiliary::Direct => None,
        Auxiliary::Socks(socks) => Some(Zeroizing::new(format!("socks5h://{socks}"))),
        Auxiliary::BuiltIn {
            rpc_url,
            token_file,
        } => {
            let deadline = std::time::Instant::now() + patience;
            loop {
                match built_in_proxy(rpc_url, token_file) {
                    Ok(proxy) => break Some(proxy),
                    Err(_) if std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_secs(5));
                    }
                    Err(error) => return Err(error.context(
                        "the built-in Tor is not ready (it starts with the node); nothing was sent directly",
                    )),
                }
            }
        }
    })
}
fn built_in_proxy(rpc_url: &str, token_file: &std::path::Path) -> Result<Zeroizing<String>> {
    ensure!(is_loopback_url(rpc_url), "only a local node is asked");
    let token = Zeroizing::new(std::fs::read_to_string(token_file)?.trim().to_owned());
    let url = reqwest::Url::parse(rpc_url)?.join("v1/network/proxy")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let response = local_client()?
                .get(url)
                .bearer_auth(token.as_str())
                .send()
                .await?;
            ensure!(response.status().is_success(), "no Tor proxy available");
            let body: Value = response.json().await?;
            Ok(Zeroizing::new(
                body["proxy"].as_str().unwrap_or_default().to_owned(),
            ))
        })
}
/// HTTP client for auxiliary traffic. A proxy must resolve host names itself
/// (`socks5h`), so no name is looked up locally.
pub(super) fn http_client(proxy: Option<&str>) -> Result<reqwest::ClientBuilder> {
    let builder = reqwest::Client::builder();
    Ok(match proxy {
        Some(proxy) => {
            ensure!(
                proxy.starts_with("socks5h://"),
                "auxiliary traffic needs a Tor proxy that resolves names"
            );
            builder.proxy(reqwest::Proxy::all(proxy)?)
        }
        None => builder,
    })
}
pub(super) const MAX_LIGHT_SOURCES: usize = 8;
/// Operations that never contact a node.
const OFFLINE_OPS: [&str; 6] = [
    "create",
    "restore",
    "open",
    "address",
    "derive_account",
    "estimate_fee",
];
/// Request fields that make the wallet CLI a light wallet for `op`, `None`
/// when this wallet uses its own node or `op` needs no node.
pub(super) fn light_fields(
    preferences: &preferences::Preferences,
    settings: &Settings,
    op: &str,
) -> Result<Option<Value>> {
    if !preferences.light_wallet || OFFLINE_OPS.contains(&op) {
        return Ok(None);
    }
    ensure!(
        matches!(op, "sync" | "send"),
        "Not available in light wallet mode yet: it needs a node of your own."
    );
    ensure!(
        !preferences.light_sources.is_empty(),
        "Add at least one remote node (an onion or HTTPS address) for the light wallet."
    );
    let mut fields = json!({
        "light": true,
        "sources": preferences.light_sources,
        "tor_dir": PathBuf::from(&settings.data_dir).join("light-tor").display().to_string(),
    });
    if Engine::of(preferences) == Engine::External {
        fields["tor_socks"] =
            json!(parse_socket(&preferences.tor_socks, "Tor SOCKS proxy")?.to_string());
    }
    Ok(Some(fields))
}
/// `genorynd` arguments for the chosen mode. Invalid settings stop the start.
pub(super) fn node_arguments(preferences: &preferences::Preferences) -> Result<Vec<String>> {
    let mode = Mode::parse(&preferences.network_mode).unwrap_or(Mode::Direct);
    let mut arguments = vec!["--network-mode".to_owned(), mode.name().to_owned()];
    if mode == Mode::Direct {
        return Ok(arguments);
    }
    if Engine::of(preferences) == Engine::External {
        let socks = parse_socket(&preferences.tor_socks, "Tor SOCKS proxy")?;
        arguments.extend(["--tor-socks".to_owned(), socks.to_string()]);
        if !preferences.tor_control.trim().is_empty() {
            let control = parse_socket(&preferences.tor_control, "Tor control port")?;
            arguments.extend(["--tor-control".to_owned(), control.to_string()]);
        }
        ensure!(
            !preferences.onion_service || !preferences.tor_control.trim().is_empty(),
            "an onion service through your own Tor needs its control port"
        );
    }
    if preferences.onion_service {
        arguments.push("--onion-service".to_owned());
    }
    Ok(arguments)
}
fn local_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?)
}
/// The node's network privacy status from its loopback administrative RPC.
pub(super) fn fetch_status(rpc_url: &str) -> Result<Value> {
    ensure!(
        is_loopback_url(rpc_url),
        "status is only read from a local node"
    );
    let url = reqwest::Url::parse(rpc_url)?.join("v1/network")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let response = local_client()?.get(url).send().await?;
            ensure!(
                response.status().is_success(),
                "this node does not report network privacy status"
            );
            Ok(response.json().await?)
        })
}
pub(super) fn request_reconnect(rpc_url: &str, token_file: &std::path::Path) -> Result<()> {
    ensure!(
        is_loopback_url(rpc_url),
        "reconnect is only sent to a local node"
    );
    let token = Zeroizing::new(std::fs::read_to_string(token_file)?.trim().to_owned());
    let url = reqwest::Url::parse(rpc_url)?.join("v1/network/reconnect")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let response = local_client()?
                .post(url)
                .bearer_auth(token.as_str())
                .json(&json!({}))
                .send()
                .await?;
            ensure!(
                response.status().is_success(),
                "the node refused the reconnect request"
            );
            Ok(())
        })
}

impl WalletApp {
    pub(super) fn retry_tor(&mut self) {
        if self.reconnect_busy {
            return;
        }
        self.reconnect_busy = true;
        let (tx, rpc) = (self.tx.clone(), self.settings.rpc_url.clone());
        let token = PathBuf::from(&self.settings.data_dir).join("admin.token");
        thread::spawn(move || {
            let result = request_reconnect(&rpc, &token).map_err(|error| error.to_string());
            let _ = tx.send(Event::Reconnect(result));
        });
    }
    /// Saves the mode, applies it to the wallet's own traffic at once and to the
    /// owned node through a single serialized restart.
    fn choose_mode(&mut self, mode: Mode) {
        self.preferences.network_mode = mode.name().into();
        set_active(mode);
        self.save_preferences();
        self.apply_network_mode(mode);
    }
    fn transition_status(&mut self, ui: &mut egui::Ui) {
        use network_transition::Outcome;
        if let Some(transition) = self.network_transition {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                motion::loader(ui, 14.0);
                ui.label(format!(
                    "Applying {}: {}…",
                    transition.target.label(),
                    transition.phase.describe()
                ));
                if ui.button("Cancel").clicked() {
                    self.cancel_network_transition();
                }
            });
            return;
        }
        match &self.network_outcome {
            Some(Outcome::Applied(mode)) => {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{} is active on the node.", mode.label()))
                        .small()
                        .color(GRAY),
                );
            }
            Some(Outcome::Failed(reason)) => {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("Not applied: {reason}")).strong());
                ui.label(
                    RichText::new("Nothing switched to a direct connection. Try again, or check the node log.")
                        .small()
                        .color(GRAY),
                );
            }
            Some(Outcome::Cancelled) | None => {}
        }
    }
    pub(super) fn network_privacy_card(&mut self, ui: &mut egui::Ui) {
        let selected = Mode::parse(&self.preferences.network_mode).unwrap_or(Mode::Direct);
        card(ui, |ui| {
            small_title(ui, "NETWORK PRIVACY");
            ui.label("Mode for the node this wallet starts");
            ui.horizontal(|ui| {
                for mode in Mode::ALL {
                    if ui
                        .add(egui::Button::new(mode.label()).selected(selected == mode))
                        .clicked()
                        && selected != mode
                    {
                        self.choose_mode(mode);
                    }
                }
            });
            ui.label(RichText::new(selected.summary()).color(GRAY));
            self.transition_status(ui);
            if let Some(warning) = selected.warning() {
                ui.add_space(6.0);
                ui.label(RichText::new(warning).strong());
            }
            if selected != Mode::Direct {
                ui.add_space(10.0);
                let engine = Engine::of(&self.preferences);
                ui.horizontal(|ui| {
                    for (choice, label) in [
                        (Engine::BuiltIn, "Built-in Tor"),
                        (Engine::External, "My own Tor"),
                    ] {
                        if ui
                            .add(egui::Button::new(label).selected(engine == choice))
                            .clicked()
                            && engine != choice
                        {
                            self.preferences.tor_engine = choice.name().into();
                            self.save_preferences();
                        }
                    }
                });
                if engine == Engine::BuiltIn {
                    ui.label(
                        RichText::new("Tor is part of GENORYN: nothing to install or keep open. It connects when the node starts, which takes a little longer the first time.")
                            .small()
                            .color(GRAY),
                    );
                } else {
                    ui.label("Tor SOCKS proxy (Tor Browser uses 127.0.0.1:9150)");
                    if ui
                        .add(field(&mut self.preferences.tor_socks).desired_width(260.0))
                        .changed()
                    {
                        self.save_preferences();
                    }
                    ui.label("Tor control port, optional (status and onion service)");
                    if ui
                        .add(
                            field(&mut self.preferences.tor_control)
                                .desired_width(260.0)
                                .hint_text("127.0.0.1:9051"),
                        )
                        .changed()
                    {
                        self.save_preferences();
                    }
                    ui.label(
                        RichText::new(
                            "GENORYN does not install or start this Tor. Keep it running.",
                        )
                        .small()
                        .color(GRAY),
                    );
                }
                if ui
                    .checkbox(
                        &mut self.preferences.onion_service,
                        "Accept peers through a Tor onion service",
                    )
                    .changed()
                {
                    self.save_preferences();
                }
                if let Err(error) = node_arguments(&self.preferences) {
                    ui.label(RichText::new(error.to_string()).strong());
                }
            }
            ui.add_space(10.0);
            if ui
                .checkbox(
                    &mut self.preferences.light_wallet,
                    "Light wallet: no node of its own, remote nodes over Tor",
                )
                .changed()
            {
                self.save_preferences();
            }
            if self.preferences.light_wallet {
                ui.label("Remote nodes, one per line (onion or HTTPS public RPC address)");
                let mut text = self.preferences.light_sources.join("\n");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_rows(3)
                            .desired_width(420.0)
                            .hint_text("http://<56 characters>.onion:29612"),
                    )
                    .changed()
                {
                    self.preferences.light_sources = text
                        .lines()
                        .map(|line| line.trim().to_owned())
                        .filter(|line| !line.is_empty())
                        .take(MAX_LIGHT_SOURCES)
                        .collect();
                    self.save_preferences();
                }
                ui.label(RichText::new(LIGHT_TRUST).small().color(GRAY));
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.node.is_some() && self.network_transition.is_none(),
                        egui::Button::new("Apply and restart node"),
                    )
                    .on_hover_text(
                        "Restarts the node this wallet owns with the saved Tor settings; a running miner pauses and resumes",
                    )
                    .clicked()
                {
                    self.restart_owned_node();
                }
                if ui
                    .add_enabled(
                        !self.reconnect_busy && self.network_privacy.is_some(),
                        egui::Button::new("Retry Tor now"),
                    )
                    .clicked()
                {
                    self.retry_tor();
                }
            });
            if self.node.is_none() {
                ui.label(
                    RichText::new("Saved. It applies the next time this wallet starts its node; a node started separately keeps its own settings.")
                        .small()
                        .color(GRAY),
                );
            }
            ui.add_space(14.0);
            small_title(ui, "RUNNING NODE");
            match &self.network_privacy {
                Some(status) => network_status(ui, status),
                None => {
                    ui.label("The connected node does not report network privacy status (an older or remote node).");
                }
            }
            ui.add_space(12.0);
            ui.label(RichText::new(LAYERS).small().color(GRAY));
            ui.label(
                RichText::new(match selected {
                    Mode::TorOnly => "Wallet traffic: only your local node; update checks go through Tor (with the built-in Tor, through your running node) and fail rather than connect directly. Links you open use your browser's own network.",
                    _ => "Wallet traffic: your configured node; update checks use direct HTTPS to api.github.com. Links you open use your browser's own network.",
                })
                .small()
                .color(GRAY),
            );
        });
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => "—".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
fn network_status(ui: &mut egui::Ui, status: &Value) {
    let mode = Mode::parse(status["mode"].as_str().unwrap_or_default());
    diagnostic(ui, "Mode", mode.map_or("Unknown", Mode::label));
    let connections = &status["connections"];
    diagnostic(
        ui,
        "Connections",
        format!(
            "{} clearnet in · {} clearnet out · {} Tor out · {} onion in",
            text(&connections["clearnet_inbound"]),
            text(&connections["clearnet_outbound"]),
            text(&connections["tor_outbound"]),
            text(&connections["onion_inbound"]),
        ),
    );
    let tor = &status["tor"];
    if tor.is_object() {
        let built_in = tor["backend"] == "built-in";
        if built_in {
            diagnostic(ui, "Tor", "Built-in client");
        } else {
            diagnostic(
                ui,
                "Tor SOCKS proxy",
                if tor["socks_reachable"] == true {
                    "Reachable"
                } else {
                    "Unreachable"
                },
            );
            let control = text(&tor["control"]);
            diagnostic(ui, "Tor control port", &control);
            if let Some(error) = tor["control_error"].as_str() {
                ui.label(RichText::new(error).small().color(GRAY));
            }
        }
        if let Some(error) = tor["last_error"].as_str() {
            ui.label(RichText::new(error).small().color(GRAY));
        }
        match tor["bootstrap_progress"].as_u64() {
            Some(progress) => {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [210.0, 20.0],
                        egui::Label::new(RichText::new("Tor bootstrap").color(GRAY)),
                    );
                    ui.add(
                        egui::ProgressBar::new(progress as f32 / 100.0)
                            .desired_width(220.0)
                            .text(format!("{progress}%")),
                    );
                });
                if let Some(summary) = tor["bootstrap_summary"].as_str() {
                    ui.label(RichText::new(summary).small().color(GRAY));
                }
            }
            None => diagnostic(ui, "Tor bootstrap", "Not reported without the control port"),
        }
        diagnostic(
            ui,
            "Tor routing",
            if tor["ready"] == true {
                "Ready"
            } else {
                "Paused until Tor is ready"
            },
        );
        let onion = &tor["onion"];
        diagnostic(ui, "Onion service", text(&onion["state"]));
        if let Some(address) = onion["address"].as_str() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(address).monospace().small());
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(address.to_owned());
                }
            });
        }
        if let Some(error) = onion["error"].as_str() {
            ui.label(RichText::new(error).small().color(GRAY));
        }
    } else {
        diagnostic(ui, "Tor", "Not used in this mode");
    }
    for warning in status["warnings"].as_array().into_iter().flatten() {
        if let Some(warning) = warning.as_str() {
            ui.label(RichText::new(warning).strong());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_and_only_tor_only_has_no_ip_warning() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
            assert_eq!(mode.warning().is_none(), mode == Mode::TorOnly);
            assert!(!mode.summary().contains("100% anonymous"));
        }
        assert!(LAYERS.contains("Neither makes anyone 100% anonymous"));
        assert_eq!(Mode::parse("Tor"), None);
    }

    #[test]
    fn light_wallet_requests_carry_sources_and_refuse_node_only_operations() {
        let settings = Settings::default();
        let mut preferences = preferences::Preferences::default();
        assert_eq!(light_fields(&preferences, &settings, "sync").unwrap(), None);
        preferences.light_wallet = true;
        assert!(
            light_fields(&preferences, &settings, "sync").is_err(),
            "no source configured"
        );
        preferences.light_sources = vec!["http://example.onion:29612".into()];
        let fields = light_fields(&preferences, &settings, "send")
            .unwrap()
            .unwrap();
        assert_eq!(fields["light"], true);
        assert_eq!(fields["sources"][0], "http://example.onion:29612");
        assert!(fields["tor_dir"].as_str().unwrap().ends_with("light-tor"));
        assert!(fields.get("tor_socks").is_none(), "built-in Tor");
        assert_eq!(light_fields(&preferences, &settings, "open").unwrap(), None);
        assert!(light_fields(&preferences, &settings, "register_name").is_err());
        preferences.tor_engine = "external".into();
        preferences.tor_socks = "127.0.0.1:9150".into();
        let fields = light_fields(&preferences, &settings, "sync")
            .unwrap()
            .unwrap();
        assert_eq!(fields["tor_socks"], "127.0.0.1:9150");
    }

    #[test]
    fn built_in_tor_needs_no_proxy_settings_and_ignores_external_ones() {
        let mut preferences = preferences::Preferences::default();
        assert_eq!(Engine::of(&preferences), Engine::BuiltIn);
        preferences.network_mode = "tor-only".into();
        preferences.tor_socks = "not an address".into();
        preferences.tor_control = "127.0.0.1:9151".into();
        assert_eq!(
            node_arguments(&preferences).unwrap(),
            ["--network-mode", "tor-only"],
            "the node uses its built-in client"
        );
        preferences.onion_service = true;
        preferences.tor_control.clear();
        assert_eq!(
            node_arguments(&preferences).unwrap(),
            ["--network-mode", "tor-only", "--onion-service"],
            "the built-in client hosts the onion service itself"
        );
    }

    #[test]
    fn node_arguments_follow_preferences_and_reject_invalid_tor_settings() {
        let mut preferences = preferences::Preferences {
            network_mode: "direct".into(),
            ..preferences::Preferences::default()
        };
        assert_eq!(
            node_arguments(&preferences).unwrap(),
            ["--network-mode", "direct"],
            "direct mode passes no Tor options"
        );
        preferences.network_mode = "tor-only".into();
        preferences.tor_engine = "external".into();
        preferences.tor_socks = "127.0.0.1:9150".into();
        preferences.tor_control = "127.0.0.1:9151".into();
        preferences.onion_service = true;
        assert_eq!(
            node_arguments(&preferences).unwrap(),
            [
                "--network-mode",
                "tor-only",
                "--tor-socks",
                "127.0.0.1:9150",
                "--tor-control",
                "127.0.0.1:9151",
                "--onion-service"
            ]
        );
        preferences.tor_socks = "localhost:9050".into();
        assert!(
            node_arguments(&preferences).is_err(),
            "host names would need DNS"
        );
        preferences.tor_socks = DEFAULT_SOCKS.into();
        preferences.tor_control.clear();
        assert!(
            node_arguments(&preferences).is_err(),
            "onion service needs control"
        );
    }

    #[test]
    fn tor_only_refuses_remote_rpc_and_routes_auxiliary_traffic_through_tor() {
        let mut preferences = preferences::Preferences {
            network_mode: "direct".into(),
            ..preferences::Preferences::default()
        };
        let mut settings = Settings {
            rpc_url: "http://127.0.0.1:1".into(),
            data_dir: std::env::temp_dir()
                .join("genoryn-no-node-here")
                .display()
                .to_string(),
            ..Settings::default()
        };
        assert_eq!(
            auxiliary_route(&preferences, &settings).unwrap(),
            Auxiliary::Direct
        );
        preferences.network_mode = "tor-only".into();
        let route = auxiliary_route(&preferences, &settings).unwrap();
        assert!(matches!(route, Auxiliary::BuiltIn { .. }));
        assert!(
            auxiliary_proxy_within(&route, Duration::ZERO).is_err(),
            "no running node: the check fails instead of going direct"
        );
        settings.rpc_url = "https://rpc.example.org".into();
        assert!(auxiliary_route(&preferences, &settings).is_err());
        preferences.tor_engine = "external".into();
        assert_eq!(
            auxiliary_route(&preferences, &settings).unwrap(),
            Auxiliary::Socks(DEFAULT_SOCKS.parse().unwrap())
        );
        assert_eq!(
            auxiliary_proxy(&Auxiliary::Socks(DEFAULT_SOCKS.parse().unwrap()))
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("socks5h://127.0.0.1:9050")
        );
        preferences.tor_socks = "not an address".into();
        assert!(
            auxiliary_route(&preferences, &settings).is_err(),
            "never silently direct"
        );
        assert!(
            http_client(Some("socks5://127.0.0.1:9050")).is_err(),
            "names would resolve locally"
        );
        assert!(http_client(Some("http://127.0.0.1:8080")).is_err());
        set_active(Mode::TorOnly);
        assert!(guard_rpc("http://127.0.0.1:29611").is_ok());
        assert!(guard_rpc("http://[::1]:29611").is_ok());
        assert!(guard_rpc("https://rpc.example.org").is_err());
        set_active(Mode::Hybrid);
        assert!(guard_rpc("https://rpc.example.org").is_ok());
        set_active(Mode::Direct);
    }
}
