//! Native wallet views node; uses the shared backend.
use super::*;

/// What the node this wallet uses is doing, from what can be observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NodeState {
    Running,
    Starting,
    Error,
    External,
    Stopped,
}
impl NodeState {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Starting => "STARTING",
            Self::Error => "NOT ANSWERING",
            Self::External => "CONNECTED TO A NODE STARTED ELSEWHERE",
            Self::Stopped => "STOPPED",
        }
    }
    fn explain(self) -> &'static str {
        match self {
            Self::Running => "Your own node is validating the chain on this computer.",
            Self::Starting => "The node is opening its database and connecting to peers.",
            Self::Error => {
                "The node this wallet started does not answer. View its log, then restart it."
            }
            Self::External => {
                "A node already answers at this address. The wallet uses it and does not start a second one on the same data."
            }
            Self::Stopped => "No node answers. Start your own full node with one click.",
        }
    }
}

/// Observations the node page keeps between frames.
#[derive(Default)]
pub(super) struct NodeView {
    pub started: Option<Instant>,
    /// `(when, height)` samples, newest last, for the synchronization label.
    pub heights: std::collections::VecDeque<(Instant, u64)>,
    pub preflight: Option<Preflight>,
    pub preflight_running: bool,
}
impl NodeView {
    pub(super) fn observe(&mut self, height: Option<u64>) {
        let Some(height) = height else { return };
        if self.heights.back().is_none_or(|(_, last)| *last != height) {
            self.heights.push_back((Instant::now(), height));
        }
        while self.heights.len() > 64 {
            self.heights.pop_front();
        }
    }
    /// Honest synchronization state: only this node's own progress is known.
    pub(super) fn sync_label(&self, peers: u64) -> String {
        if peers == 0 {
            return "No peers yet: it cannot learn new blocks until it connects.".into();
        }
        let recent = self
            .heights
            .iter()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(60))
            .count();
        if recent >= 3 {
            "Synchronizing: new blocks are being validated now.".into()
        } else {
            "Following its peers (no burst of new blocks in the last minute). Whether peers know a longer chain cannot be proven from here.".into()
        }
    }
    pub(super) fn recent_errors(&self, log: Option<&str>) -> Vec<String> {
        log.map(|text| {
            text.lines()
                .filter(|line| line.contains(" ERROR") || line.contains(" WARN"))
                .rev()
                .take(5)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreflightLevel {
    Ok,
    Warning,
    Failed,
    Info,
}
impl PreflightLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "Ready",
            Self::Warning => "Warning",
            Self::Failed => "Blocked",
            Self::Info => "Setting",
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct PreflightItem {
    pub name: &'static str,
    pub level: PreflightLevel,
    pub detail: String,
}
/// Checks made before the node starts; a failed one blocks the start.
#[derive(Clone, Debug, Default)]
pub(super) struct Preflight {
    pub items: Vec<PreflightItem>,
}
/// Free space below which the node is not started (the chain is about 60 MB
/// today and grows by about 12 KB per block).
const MIN_FREE_BYTES: u64 = 200 * 1024 * 1024;
const SUGGESTED_FREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
impl Preflight {
    pub(super) fn blocking_problem(&self) -> Option<String> {
        self.items
            .iter()
            .find(|item| item.level == PreflightLevel::Failed)
            .map(|item| format!("Cannot start: {}", item.detail))
    }
    pub(super) fn run(
        settings: &Settings,
        preferences: &preferences::Preferences,
        answering: bool,
    ) -> Self {
        let mut items = Vec::new();
        let supported = matches!(
            (std::env::consts::OS, std::env::consts::ARCH),
            ("windows" | "linux", "x86_64")
        );
        items.push(PreflightItem {
            name: "Platform",
            level: if supported {
                PreflightLevel::Ok
            } else {
                PreflightLevel::Warning
            },
            detail: format!(
                "{} {} ({})",
                std::env::consts::OS,
                std::env::consts::ARCH,
                if supported {
                    "a tested platform"
                } else {
                    "not tested by GENORYN releases"
                }
            ),
        });
        let directory = PathBuf::from(&settings.data_dir);
        let existing = directory
            .ancestors()
            .find(|path| path.exists())
            .map(PathBuf::from);
        items.push(
            match existing
                .as_ref()
                .and_then(|path| fs2::available_space(path).ok())
            {
                Some(free) => PreflightItem {
                    name: "Disk space",
                    level: if free < MIN_FREE_BYTES {
                        PreflightLevel::Failed
                    } else if free < SUGGESTED_FREE_BYTES {
                        PreflightLevel::Warning
                    } else {
                        PreflightLevel::Ok
                    },
                    detail: format!("{:.1} GB free where the chain is stored", free as f64 / 1e9),
                },
                None => PreflightItem {
                    name: "Disk space",
                    level: PreflightLevel::Warning,
                    detail: "Could not measure free space.".into(),
                },
            },
        );
        items.push(match total_memory() {
            Some(bytes) => PreflightItem {
                name: "Memory",
                level: if bytes < 2 * 1024 * 1024 * 1024 {
                    PreflightLevel::Warning
                } else {
                    PreflightLevel::Ok
                },
                detail: format!(
                    "{:.1} GB installed; the node needs about 0.5 GB (one 256 MB RandomX cache)",
                    bytes as f64 / 1e9
                ),
            },
            None => PreflightItem {
                name: "Memory",
                level: PreflightLevel::Info,
                detail: "Not measured on this system; the node needs about 0.5 GB.".into(),
            },
        });
        items.push(
            match protocol::sibling_binary("genorynd", &settings.bin_dir) {
                Ok(binary) => {
                    let runs = protocol::gui_child_command(&binary)
                        .arg("--help")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|status| status.success());
                    PreflightItem {
                        name: "Node program",
                        level: if runs {
                            PreflightLevel::Ok
                        } else {
                            PreflightLevel::Failed
                        },
                        detail: if runs {
                            format!("{} runs", binary.display())
                        } else {
                            format!("{} does not run on this computer", binary.display())
                        },
                    }
                }
                Err(error) => PreflightItem {
                    name: "Node program",
                    level: PreflightLevel::Failed,
                    detail: error.to_string(),
                },
            },
        );
        let writable = std::fs::create_dir_all(&directory)
            .and_then(|()| {
                let probe = directory.join(".genoryn-write-check");
                std::fs::write(&probe, b"ok")?;
                std::fs::remove_file(probe)
            })
            .is_ok();
        let mainnet = directory
            .display()
            .to_string()
            .contains(&NETWORK.data_dir_name());
        items.push(PreflightItem {
            name: "Data folder",
            level: if !writable {
                PreflightLevel::Failed
            } else if mainnet {
                PreflightLevel::Ok
            } else {
                PreflightLevel::Warning
            },
            detail: format!(
                "{}{}",
                directory.display(),
                if !writable {
                    " cannot be written"
                } else if mainnet {
                    " (Mainnet 1.0 folder)"
                } else {
                    " (not the usual Mainnet 1.0 folder; a node refuses another network's database)"
                }
            ),
        });
        let port_free = genoryn_connect::validate_endpoint(&settings.rpc_url, NETWORK.name())
            .ok()
            .and_then(|url| {
                Some((
                    url.host_str()?
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()?,
                    url.port_or_known_default()?,
                ))
            })
            .map(|(ip, port)| std::net::TcpListener::bind((ip, port)).is_ok());
        items.push(match (answering, port_free) {
            (true, _) => PreflightItem {
                name: "Existing node",
                level: PreflightLevel::Info,
                detail: format!(
                    "A node already answers at {}; it is used instead of starting another.",
                    settings.rpc_url
                ),
            },
            (false, Some(false)) => PreflightItem {
                name: "RPC port",
                level: PreflightLevel::Failed,
                detail: format!(
                    "Another program uses the port of {} but does not answer as a GENORYN node.",
                    settings.rpc_url
                ),
            },
            (false, Some(true)) => PreflightItem {
                name: "RPC port",
                level: PreflightLevel::Ok,
                detail: format!("{} is free (loopback only)", settings.rpc_url),
            },
            (false, None) => PreflightItem {
                name: "RPC port",
                level: PreflightLevel::Failed,
                detail: "The RPC address must be a loopback IP and port.".into(),
            },
        });
        let mode = network_privacy::Mode::parse(&preferences.network_mode)
            .unwrap_or(network_privacy::Mode::Direct);
        items.push(PreflightItem {
            name: "Network mode",
            level: PreflightLevel::Info,
            detail: format!("{} — {}", mode.label(), mode.summary()),
        });
        Self { items }
    }
}

/// Installed memory, where the system reports it without extra privileges.
fn total_memory() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kib: u64 = text
            .lines()
            .find(|l| l.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        Some(kib * 1024)
    }
    #[cfg(target_os = "windows")]
    {
        let output = protocol::gui_child_command(std::path::Path::new("powershell"))
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

pub(super) fn format_duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3599 => format!("{} min", seconds / 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

fn open_folder(directory: &str) {
    let _ = std::fs::create_dir_all(directory);
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(target_os = "windows"))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(directory).spawn();
}

const BACKGROUND_PID: &str = "genorynd.background.pid";
/// Leaves the owned node running (the user agreed) and records its process id.
pub(super) fn leave_running(data_dir: &str, child: OwnedChild) {
    if let Some(pid) = child.release() {
        let _ = std::fs::write(
            PathBuf::from(data_dir).join(BACKGROUND_PID),
            pid.to_string(),
        );
    }
}
fn background_pid(data_dir: &str) -> Option<u32> {
    std::fs::read_to_string(PathBuf::from(data_dir).join(BACKGROUND_PID))
        .ok()?
        .trim()
        .parse()
        .ok()
}
/// Stops a node this wallet left running, after checking that the recorded
/// process is still a GENORYN node (process ids are reused).
fn stop_background(data_dir: &str, pid: u32) -> Result<()> {
    let record = PathBuf::from(data_dir).join(BACKGROUND_PID);
    #[cfg(target_os = "windows")]
    {
        let listing = protocol::gui_child_command(std::path::Path::new("tasklist"))
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()?;
        if !String::from_utf8_lossy(&listing.stdout)
            .to_ascii_lowercase()
            .contains("genorynd")
        {
            let _ = std::fs::remove_file(&record);
            anyhow::bail!("process {pid} is no longer a GENORYN node; nothing was stopped");
        }
        let status = protocol::gui_child_command(std::path::Path::new("taskkill"))
            .args(["/PID", &pid.to_string()])
            .status()?;
        ensure!(status.success(), "the node did not stop");
    }
    #[cfg(not(target_os = "windows"))]
    {
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if !name.trim().starts_with("genorynd") {
            let _ = std::fs::remove_file(&record);
            anyhow::bail!("process {pid} is no longer a GENORYN node; nothing was stopped");
        }
        let status = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status()?;
        ensure!(status.success(), "the node did not stop");
    }
    let _ = std::fs::remove_file(record);
    Ok(())
}

/// The per-user sign-in start entry for the node.
fn autostart_entry() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|appdata| {
            PathBuf::from(appdata)
                .join("Microsoft\\Windows\\Start Menu\\Programs\\Startup")
                .join("GENORYN Full Node.cmd")
        })
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".config/autostart")
                .join("genoryn-full-node.desktop")
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        None
    }
}
fn install_autostart(
    settings: &Settings,
    preferences: &preferences::Preferences,
) -> Result<PathBuf> {
    let entry = autostart_entry().context("sign-in start is not available on this system")?;
    let binary = protocol::sibling_binary("genorynd", &settings.bin_dir)?;
    let url = genoryn_connect::validate_endpoint(&settings.rpc_url, NETWORK.name())?;
    let rpc = format!(
        "{}:{}",
        url.host_str().context("RPC host")?,
        url.port_or_known_default()
            .unwrap_or(NETWORK.base_port() + 1)
    );
    ensure!(
        network_privacy::is_loopback_url(&settings.rpc_url),
        "the node's RPC must stay on loopback"
    );
    let mut arguments = vec![
        "--network".to_owned(),
        NETWORK.name().to_owned(),
        "--data-dir".to_owned(),
        settings.data_dir.clone(),
        "--rpc-listen".to_owned(),
        rpc,
    ];
    arguments.extend(network_privacy::node_arguments(preferences)?);
    let log = PathBuf::from(&settings.data_dir).join("genorynd.log");
    let quote = |text: &str| format!("\"{}\"", text.replace('"', ""));
    if let Some(parent) = entry.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(target_os = "windows")]
    let contents = format!(
        "@echo off\r\nrem GENORYN full node, added by the wallet at your request. Delete this file to stop starting it.\r\nstart \"GENORYN node\" /min cmd /c \"{} {} >> {} 2>&1\"\r\n",
        quote(&binary.display().to_string()),
        arguments
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(" "),
        quote(&log.display().to_string()),
    );
    #[cfg(not(target_os = "windows"))]
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=GENORYN Full Node\nComment=Added by the GENORYN wallet at your request\nExec=sh -c '{} {} >> {} 2>&1'\nX-GNOME-Autostart-enabled=true\n",
        quote(&binary.display().to_string()),
        arguments
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(" "),
        quote(&log.display().to_string()),
    );
    std::fs::write(&entry, contents)?;
    Ok(entry)
}

/// Average time for this computer to find a block, and its share of the network,
/// from its hashrate and the next block's difficulty (expected hashes per block).
pub(super) fn block_odds(hashrate: f64, difficulty: f64) -> Option<String> {
    if !(hashrate > 0.0 && difficulty > 0.0) {
        return None;
    }
    let seconds = difficulty / hashrate;
    let share = (hashrate / (difficulty / 60.0) * 100.0).min(100.0);
    let wait = if seconds < 90.0 {
        format!("{seconds:.0} seconds")
    } else if seconds < 5400.0 {
        format!("{:.0} minutes", seconds / 60.0)
    } else if seconds < 172_800.0 {
        format!("{:.1} hours", seconds / 3600.0)
    } else {
        format!("{:.1} days", seconds / 86_400.0)
    };
    Some(format!(
        "This computer finds a block about every {wait} on average · about {share:.1}% of the network"
    ))
}
impl WalletApp {
    pub(super) fn mining(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Mining",
            "Solo or shared CPU mining. It starts only when you choose.",
        );
        ui.add_space(20.0);
        self.mining_mode_card(ui);
        ui.add_space(18.0);
        card(ui, |ui| {
            ui.label(
                RichText::new(if self.miner.is_some() {
                    "MINER RUNNING"
                } else {
                    "MINER STOPPED"
                })
                .size(22.0),
            );
            ui.add_space(16.0);
            ui.label("Mining destination");
            match (&self.miner_destination, self.miner.is_some()) {
                (Some((address, name)), true) => {
                    ui.label(RichText::new(format!("Wallet “{name}”")).strong());
                    ui.label(address.as_str());
                }
                _ => {
                    ui.label(if self.unlocked {
                        self.address.as_str()
                    } else {
                        "Unlock your wallet to select its receiving address."
                    });
                }
            }
            if let Some(name) = self.mining_elsewhere() {
                ui.label(
                    RichText::new(format!(
                        "The miner pays wallet “{name}”, not the wallet open now. Switch to “{name}” in the wallet menu to see its rewards, or stop and start mining to pay this wallet."
                    ))
                    .color(ERROR),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label("CPU threads");
                // Matches genoryn-miner in its default fast mode: one worker per logical CPU.
                let max = std::thread::available_parallelism()
                    .map(|n| n.get() as u32)
                    .unwrap_or(1)
                    .min(256);
                ui.add_enabled(
                    self.miner.is_none(),
                    egui::DragValue::new(&mut self.settings.threads).range(1..=max),
                );
                ui.label(
                    RichText::new(format!(
                        "of {max} · about 2.3 GiB of memory, shared by all threads"
                    ))
                    .small()
                    .color(GRAY),
                );
            });
            ui.label("Fast RandomX mode: one 2 GiB dataset for all threads, ready about a minute after mining starts. Until then, or if it cannot be allocated, the miner uses light mode: 256 MiB per thread, at most 16 threads.");
            ui.add_space(14.0);
            let shared = self.preferences.mining_mode == "pool";
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.unlocked
                            && (self.node_info.is_some() || shared)
                            && self.miner.is_none()
                            && !self.process_busy,
                        primary(
                            ui,
                            if shared {
                                "Start shared mining"
                            } else {
                                "Start CPU mining"
                            },
                        )
                        .min_size(egui::vec2(200.0, 44.0)),
                    )
                    .clicked()
                {
                    self.start_mining();
                }
                if ui
                    .add_enabled(
                        self.miner.is_some() && !self.process_busy,
                        egui::Button::new("Stop mining"),
                    )
                    .clicked()
                {
                    self.stop_mining();
                }
            });
            if let Some(note) = &self.miner_note {
                ui.add_space(14.0);
                ui.label(RichText::new(note.as_str()).strong());
                ui.label(
                    RichText::new(format!(
                        "Miner log: {}",
                        PathBuf::from(&self.settings.data_dir)
                            .join("genoryn-miner.log")
                            .display()
                    ))
                    .small()
                    .color(GRAY),
                );
            }
            if self.miner_retry_at.is_some() {
                ui.label(RichText::new("Applying the fix and restarting the miner...").small());
            } else if self.miner.is_some() && self.miner_attempt.retries > 0 {
                ui.label(
                    RichText::new(format!(
                        "Running with the automatic fix: {} thread(s), RandomX {} mode.",
                        self.miner_attempt.threads,
                        if self.miner_attempt.portable {
                            "portable"
                        } else {
                            "optimized"
                        }
                    ))
                    .small(),
                );
            }
            ui.add_space(18.0);
            ui.label("Mining uses CPU and electricity. Rewards mature after 60 blocks.");
        });
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "LIVE MINER METRICS");
            if let Some(stats) = &self.mining_stats {
                let difficulty = self
                    .node_info
                    .as_ref()
                    .and_then(|node| node.info.difficulty.parse::<f64>().ok());
                if let Some(text) = stats["hashrate_hs"]
                    .as_f64()
                    .zip(difficulty)
                    .filter(|_| stats["event"] == "mining")
                    .and_then(|(rate, difficulty)| block_odds(rate, difficulty))
                {
                    ui.label(RichText::new(text).strong());
                    ui.label(
                        RichText::new("Blocks arrive at random. With more miners the difficulty rises and blocks take longer; mining is still working while you wait.")
                            .small()
                            .color(GRAY),
                    );
                    ui.add_space(8.0);
                }
                let rows: &[(&str, &str)] = if stats["event"] == "pool_mining" {
                    &[
                        ("hashrate_hs", "Hashrate / second"),
                        ("hashes", "Hashes attempted"),
                        ("accepted_shares", "Shares accepted by the pool"),
                        ("accepted_work", "Verified work credited (expected hashes)"),
                        ("rejected_shares", "Shares rejected"),
                        ("last_reject", "Last rejection"),
                        ("share_difficulty", "Share difficulty"),
                        ("block_candidates", "Shares that were blocks"),
                        ("height", "Job height"),
                    ]
                } else {
                    &[
                        ("hashrate_hs", "Hashrate / second"),
                        ("hashes", "Hashes attempted"),
                        ("accepted_blocks", "Blocks accepted"),
                        ("rejected_blocks", "Blocks rejected"),
                        ("height", "Current height"),
                    ]
                };
                for (key, label) in rows.iter().copied() {
                    ui.label(format!(
                        "{label}: {}",
                        stats
                            .get(key)
                            .map(Value::to_string)
                            .unwrap_or_else(|| "Not reported".into())
                    ));
                }
            } else {
                ui.label("No live metrics received. Start the miner to measure this computer.");
            }
        });
    }
    fn mining_mode_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            small_title(ui, "HOW TO MINE");
            let idle = self.miner.is_none() && !self.process_busy;
            ui.horizontal(|ui| {
                for (mode, label) in [("solo", "Solo"), ("pool", "Shared (pool)")] {
                    if ui
                        .add_enabled(
                            idle,
                            egui::Button::new(label).selected(self.preferences.mining_mode == mode),
                        )
                        .clicked()
                        && self.preferences.mining_mode != mode
                    {
                        self.preferences.mining_mode = mode.into();
                        self.save_preferences();
                    }
                }
            });
            if self.preferences.mining_mode != "pool" {
                ui.label("Solo: your own node builds the block template and the whole reward goes to your wallet when this computer finds a block. A slow computer may wait a long time between blocks.");
                return;
            }
            ui.label("Shared: a pool hands out work, checks every share you find with real RandomX, and when the pool finds a block divides its reward by verified work (PPLNS). A 10 H/s computer earns in proportion to its 10 H/s.");
            ui.add_space(8.0);
            ui.label("Pool address");
            if ui
                .add_enabled(
                    idle,
                    field(&mut self.preferences.pool_url)
                        .hint_text("tls://pool.example.org:29790  or  tcp://…onion:29790")
                        .desired_width(f32::INFINITY)
                        .char_limit(300),
                )
                .changed()
            {
                self.save_preferences();
            }
            ui.label("Tor proxy for the pool (optional; in Tor-only mode your node's Tor is used)");
            if ui
                .add_enabled(
                    idle,
                    field(&mut self.preferences.pool_proxy)
                        .hint_text("socks5h://127.0.0.1:9050")
                        .desired_width(360.0)
                        .char_limit(200),
                )
                .changed()
            {
                self.save_preferences();
            }
            ui.add_space(8.0);
            ui.label(RichText::new("How a pool reward reaches you").strong());
            for line in [
                "1. Accepted shares — verified work the pool recorded. Not money yet.",
                "2. Immature — the pool found a block; its reward is not spendable for 60 blocks.",
                "3. Matured credit — your exact share, kept by the pool (fractions below one gori too).",
                "4. Pending payout — in a private payout transaction to your address.",
                "5. Received — the payment confirmed on the chain; only now it is in your balance.",
            ] {
                ui.label(RichText::new(line).small());
            }
            ui.label(RichText::new("Custody: a pool keeps mined rewards in its own wallet until it pays them out, and can fail or keep them. Choose a pool you trust, or mine solo. GENORYN does not operate or endorse any pool; anyone can run one (genoryn-pool, open source).").small().color(GRAY));
        });
    }
    pub(super) fn node_page(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "My Full Node",
            "Validate every block yourself. Nobody has to be trusted for your balance.",
        );
        ui.add_space(20.0);
        self.node_view
            .observe(self.node_info.as_ref().map(|n| n.info.height));
        if self.node_view.preflight.is_none() && !self.node_view.preflight_running {
            self.start_preflight();
        }
        let state = self.node_state();
        card(ui, |ui| {
            ui.label(RichText::new(state.title()).size(22.0));
            ui.label(RichText::new(state.explain()).color(GRAY));
            ui.add_space(16.0);
            let blocked = self
                .node_view
                .preflight
                .as_ref()
                .and_then(Preflight::blocking_problem);
            ui.horizontal(|ui| {
                if layout_probe::tag(
                    "node:start",
                    ui.add_enabled(
                        self.node.is_none()
                            && self.node_info.is_none()
                            && !self.process_busy
                            && blocked.is_none(),
                        primary(ui, "Start My Full Node").min_size(egui::vec2(220.0, 44.0)),
                    ),
                )
                .clicked()
                {
                    self.start_process("node");
                }
                if layout_probe::tag(
                    "node:stop",
                    ui.add_enabled(
                        self.node.is_some() && !self.process_busy && self.miner.is_none(),
                        egui::Button::new("Stop Node"),
                    ),
                )
                .clicked()
                {
                    self.stop_process("node");
                }
                if layout_probe::tag(
                    "node:restart",
                    ui.add_enabled(
                        self.node.is_some() && self.miner.is_none() && !self.process_busy,
                        egui::Button::new("Restart Node"),
                    ),
                )
                .clicked()
                {
                    self.restart_owned_node();
                }
            });
            if let Some(problem) = blocked {
                ui.label(RichText::new(problem).color(ERROR));
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open Node Folder").clicked() {
                    open_folder(&self.settings.data_dir);
                }
                if layout_probe::tag(
                    "node:logs",
                    ui.add_enabled(!self.log_busy, egui::Button::new("View Node Logs")),
                )
                .clicked()
                {
                    self.load_node_log();
                }
                if ui.button("Node Settings").clicked() {
                    self.page = 7;
                }
                if ui.button("Connection Diagnostics").clicked() {
                    self.refresh_network();
                    self.start_preflight();
                    self.start_security_check();
                    self.page = SECURITY_PAGE;
                }
            });
            ui.add_space(12.0);
            ui.label(RichText::new("Starting never opens an inbound port, changes the firewall or installs a service. Only a node this wallet started can be stopped here; a node started elsewhere is used as it is. Stop mining before stopping your node.").small().color(GRAY));
        });
        ui.add_space(18.0);
        self.preflight_card(ui);
        ui.add_space(18.0);
        self.background_card(ui);
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "NODE STATUS");
            if let Some(node) = &self.node_info {
                diagnostic(ui, "Status", state.title());
                diagnostic(
                    ui,
                    "Synchronization",
                    self.node_view.sync_label(node.info.peer_count),
                );
                diagnostic(ui, "Verified height", node.info.height);
                diagnostic(
                    ui,
                    "Network identity",
                    if node.info.chain_id == NETWORK.chain_id()
                        && node.info.genesis_hash == hex::encode(NETWORK.genesis())
                    {
                        format!(
                            "{} · genesis {}… (matches the built-in Mainnet 1.0)",
                            node.info.chain_id,
                            &node.info.genesis_hash[..12]
                        )
                    } else {
                        "DIFFERENT NETWORK OR GENESIS".into()
                    },
                );
                if let Some(started) = self.node_view.started.filter(|_| self.node.is_some()) {
                    diagnostic(ui, "Uptime", format_duration(started.elapsed()));
                }
                diagnostic(ui, "Peer count", node.info.peer_count);
                diagnostic(
                    ui,
                    "Network privacy",
                    self.network_privacy
                        .as_ref()
                        .and_then(|status| status["mode"].as_str())
                        .and_then(network_privacy::Mode::parse)
                        .map(|mode| match mode.warning() {
                            Some(_) => {
                                format!("{} · peers can see this computer's IP", mode.label())
                            }
                            None => {
                                format!("{} · see the Network page for Tor status", mode.label())
                            }
                        })
                        .unwrap_or_else(|| "Not reported by this node".into()),
                );
                diagnostic(
                    ui,
                    "Inbound / outbound",
                    match (node.info.inbound_peers, node.info.outbound_peers) {
                        (Some(inbound), Some(outbound)) => format!("{inbound} / {outbound}"),
                        _ => "Not reported".into(),
                    },
                );
                diagnostic(
                    ui,
                    "Chain + registry storage",
                    node.info
                        .database_bytes
                        .map(|bytes| format!("{bytes} bytes (including WAL/SHM)"))
                        .unwrap_or_else(|| "Not reported".into()),
                );
                diagnostic(ui, "Mempool transactions", node.info.mempool_count);
                diagnostic(ui, "Difficulty", &node.info.difficulty);
                diagnostic(
                    ui,
                    "Target-implied hash rate",
                    node.info
                        .estimated_hashrate_hs
                        .filter(|rate| rate.is_finite() && *rate >= 0.0)
                        .map(|rate| format!("{rate:.4} H/s"))
                        .unwrap_or_else(|| "Not reported".into()),
                );
                ui.label(RichText::new("Hash rate is next difficulty / the 60-second target, not measured miner telemetry.").small().color(GRAY));
                diagnostic(ui, "Round-trip latency", format!("{} ms", node.latency_ms));
                diagnostic(
                    ui,
                    "Public RPC",
                    if node.info.public_rpc {
                        "Enabled"
                    } else {
                        "Disabled"
                    },
                );
                ui.label(
                    RichText::new(format!("Best block: {}", node.info.best_hash))
                        .monospace()
                        .small(),
                );
                ui.label(RichText::new("Sync percentage is unknown without an independently verified target height. Peer claims do not establish global sync. Peer counts are this node's own connections, not the size of the network.").small().color(GRAY));
                let errors = self.node_view.recent_errors(self.node_log.as_deref());
                if !errors.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Recent warnings and errors (from the opened log)")
                            .small()
                            .color(GRAY),
                    );
                    for line in errors {
                        ui.add(egui::Label::new(RichText::new(line).monospace().size(11.5)).wrap());
                    }
                }
            } else {
                ui.label("No verified response from the configured node.");
                if let Some(error) = &self.network_error {
                    ui.label(RichText::new(error).color(GRAY));
                }
            }
        });
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "HELP THE NETWORK (OPTIONAL)");
            ui.label("A node only you can reach still validates everything for you. To also serve others:");
            for line in [
                "Reachable node: forward the P2P port (Mainnet 29710) to this computer and start the node with a public --p2p-listen address. Your IP address becomes public.",
                "Tor onion node: choose Tor only or Hybrid on the Network page and enable \"Accept peers through a Tor onion service\". Peers reach you without learning your IP.",
                "Public read-only RPC (a Quick Wallet source): start the node with --public-rpc. It serves blocks and accepts transactions; the admin RPC and its token are never exposed.",
            ] {
                ui.label(format!("·  {line}"));
            }
            ui.label(RichText::new("Peer discovery only finds peers; it never decides validity. Two nodes on different IP addresses are not independent if one operator runs both.").small().color(GRAY));
        });
    }
    fn node_state(&self) -> NodeState {
        let answering = self.node_info.is_some();
        match (&self.node, answering) {
            (Some(_), true) => NodeState::Running,
            (Some(_), false)
                if self
                    .node_view
                    .started
                    .is_some_and(|s| s.elapsed() < Duration::from_secs(120)) =>
            {
                NodeState::Starting
            }
            (Some(_), false) => NodeState::Error,
            (None, true) => NodeState::External,
            (None, false) => NodeState::Stopped,
        }
    }
    pub(super) fn start_preflight(&mut self) {
        if self.node_view.preflight_running {
            return;
        }
        self.node_view.preflight_running = true;
        let (tx, settings, preferences) = (
            self.tx.clone(),
            self.settings.clone(),
            self.preferences.clone(),
        );
        let answering = self.node_info.is_some();
        thread::spawn(move || {
            let _ = tx.send(Event::Preflight(Box::new(Preflight::run(
                &settings,
                &preferences,
                answering,
            ))));
        });
    }
    fn preflight_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            small_title(ui, "BEFORE STARTING");
            match &self.node_view.preflight {
                None => {
                    ui.horizontal(|ui| {
                        motion::loader(ui, 14.0);
                        ui.label("Checking this computer…");
                    });
                }
                Some(preflight) => {
                    for item in &preflight.items {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [96.0, 20.0],
                                egui::Label::new(RichText::new(item.level.label()).color(
                                    if item.level == PreflightLevel::Failed {
                                        ERROR
                                    } else {
                                        GRAY
                                    },
                                )),
                            );
                            ui.add_sized(
                                [170.0, 20.0],
                                egui::Label::new(RichText::new(item.name).strong()),
                            );
                            ui.add(egui::Label::new(item.detail.as_str()).wrap());
                        });
                    }
                    if ui.small_button("Check again").clicked() {
                        self.node_view.preflight = None;
                    }
                }
            }
        });
    }
    fn background_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            small_title(ui, "RUNNING WITHOUT THE WALLET (OPTIONAL)");
            if ui
                .checkbox(
                    &mut self.preferences.node_keep_running,
                    "Keep my node running after I close the wallet",
                )
                .changed()
            {
                self.save_preferences();
            }
            ui.label(RichText::new("Off by default. When on, the node this wallet started keeps validating after the window closes; its process id is written to the node folder so the wallet can stop it later.").small().color(GRAY));
            if let Some(pid) = background_pid(&self.settings.data_dir)
                && self.node.is_none()
            {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "A node this wallet left running (process {pid}) is recorded."
                    ));
                    if ui.button("Stop that node").clicked() {
                        self.status = match stop_background(&self.settings.data_dir, pid) {
                            Ok(()) => "The background node was stopped.".into(),
                            Err(error) => error.to_string(),
                        };
                        self.last_health = Instant::now() - Duration::from_secs(10);
                    }
                });
            }
            ui.add_space(8.0);
            let entry = autostart_entry();
            let installed = entry.as_ref().is_some_and(|path| path.is_file());
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !installed && entry.is_some(),
                        egui::Button::new("Start my node when I sign in"),
                    )
                    .clicked()
                {
                    self.status = match install_autostart(&self.settings, &self.preferences) {
                        Ok(path) => format!(
                            "Added {}. Your node starts when you sign in.",
                            path.display()
                        ),
                        Err(error) => error.to_string(),
                    };
                }
                if ui
                    .add_enabled(installed, egui::Button::new("Remove sign-in start"))
                    .clicked()
                {
                    self.status = match entry.as_ref().map(std::fs::remove_file) {
                        Some(Ok(())) => "Sign-in start removed.".into(),
                        Some(Err(error)) => error.to_string(),
                        None => "Not available on this system.".into(),
                    };
                }
            });
            ui.label(RichText::new(match &entry {
                Some(path) => format!("Uses your own sign-in start folder ({}); no administrator rights, no system service, no open ports.", path.display()),
                None => "Sign-in start is not available on this system.".into(),
            }).small().color(GRAY));
        });
    }
    pub(super) fn network(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Network",
            "Public monetary verifiability. Private ownership.",
        );
        ui.add_space(20.0);
        self.network_privacy_card(ui);
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "PINNED NETWORK IDENTITY");
            diagnostic(ui, "Chain ID", NETWORK.chain_id());
            ui.label(
                RichText::new(format!("Genesis: {}", hex::encode(NETWORK.genesis())))
                    .monospace()
                    .small(),
            );
            ui.label("A node with a different chain ID or genesis is rejected.");
        });
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "PUBLIC SUPPLY");
            if let Some(supply) = &self.supply {
                for (key, label) in [
                    ("max_supply", "Maximum supply"),
                    ("issued_supply", "Issued supply"),
                    ("remaining_supply", "Remaining supply"),
                    ("normal_mining_issued", "Normal mining issued"),
                    ("early_issued", "Early mining issued (blocks 1-500)"),
                    ("treasury_issued", "Treasury issued (burned on mainnet)"),
                    ("current_block_reward", "Current block reward"),
                ] {
                    let value = supply
                        .get(key)
                        .and_then(Value::as_str)
                        .map(format_atomic)
                        .unwrap_or_else(|| "Not reported".into());
                    diagnostic(ui, label, format!("{value} GNR"));
                }
            } else {
                ui.label("Supply data is unavailable until a matching node responds.");
            }
        });
        ui.add_space(16.0);
        ui.label(RichText::new("RPC health checks verify endpoint identity claims, not consensus. Run your own full node for independent validation.").color(GRAY));
    }
    fn updates_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            small_title(ui, "UPDATES");
            ui.label(RichText::new(format!("Installed version {}", updates::CURRENT)).strong());
            let (text, available) = match &self.update_status {
                updates::UpdateStatus::NotChecked => ("Not checked yet.".to_owned(), None),
                updates::UpdateStatus::Checking => ("Checking GitHub Releases…".to_owned(), None),
                updates::UpdateStatus::UpToDate => {
                    ("You are running the latest release.".to_owned(), None)
                }
                updates::UpdateStatus::Available(release) => (
                    format!("GENORYN {} is available.", release.version),
                    Some(release.clone()),
                ),
                updates::UpdateStatus::Failed(error) => {
                    (format!("Update check failed: {error}"), None)
                }
            };
            ui.label(RichText::new(text).color(GRAY));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.update_status != updates::UpdateStatus::Checking,
                        egui::Button::new("Check for updates"),
                    )
                    .clicked()
                {
                    self.check_for_updates();
                }
                if let Some(release) = &available
                    && ui.button(format!("Download {}", release.version)).clicked()
                {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab(release.download_url()));
                }
                if ui.button("All releases").clicked() {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab(updates::RELEASES_PAGE));
                }
            });
            if let Some(release) = available.filter(|release| !release.notes.trim().is_empty()) {
                ui.add_space(8.0);
                ui.label(RichText::new("What's new").small().color(GRAY));
                egui::ScrollArea::vertical()
                    .id_salt("release_notes")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.label(release.notes.as_str());
                    });
            }
            ui.add_space(8.0);
            if ui
                .checkbox(
                    &mut self.preferences.check_updates_at_launch,
                    "Check for updates when the wallet opens",
                )
                .changed()
            {
                self.save_preferences();
            }
            ui.label(
                RichText::new(
                    "Reads only the public latest-release record from api.github.com; nothing about your wallet is sent. Downloads open in your browser and nothing is installed automatically.",
                )
                .small()
                .color(GRAY),
            );
        });
    }
    pub(super) fn update_notice(&mut self, ctx: &egui::Context) {
        let updates::UpdateStatus::Available(release) = &self.update_status else {
            return;
        };
        if self.update_dismissed {
            return;
        }
        let release = release.clone();
        egui::Area::new(egui::Id::new("update_notice"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -64.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_gray(if self.light { 255 } else { 22 }))
                    .stroke(Stroke::new(
                        1.0,
                        Color32::from_gray(if self.light { 190 } else { 70 }),
                    ))
                    .corner_radius(10.0)
                    .inner_margin(18.0)
                    .show(ui, |ui| {
                        ui.set_max_width(330.0);
                        ui.label(RichText::new("UPDATE AVAILABLE").small().color(GRAY));
                        ui.label(
                            RichText::new(format!("GENORYN {}", release.version))
                                .size(18.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "You are running {}. Download the new release to stay compatible with the network.",
                                updates::CURRENT
                            ))
                            .color(GRAY),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("Download update").clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(release.download_url()));
                            }
                            if ui.button("Later").clicked() {
                                self.update_dismissed = true;
                            }
                        });
                    });
            });
    }
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Settings", "Explicit connections. No telemetry.");
        ui.add_space(20.0);
        card(ui, |ui| {
            ui.add_enabled_ui(
                self.busy.is_none()
                    && self.node.is_none()
                    && self.miner.is_none()
                    && !self.process_busy,
                |ui| {
                    ui.label("RPC endpoint (HTTPS, or literal loopback HTTP to your own node)");
                    if ui
                        .add(
                            field(&mut self.settings.rpc_url)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        self.node_info = None;
                        self.supply = None;
                        self.proofs = payment_proof::ProofUi::default();
                    }
                    ui.add_space(10.0);
                    ui.label("Node data directory");
                    if ui.add(
                        field(&mut self.settings.data_dir)
                            .desired_width(f32::INFINITY),
                    ).changed() { self.close_node_log(); }
                    ui.add_space(10.0);
                    ui.label("Binary directory (empty = next to this application)");
                    ui.add(
                        field(&mut self.settings.bin_dir)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(10.0);
                    ui.label("Optional node TOML configuration file");
                    ui.add(field(&mut self.settings.node_config).desired_width(f32::INFINITY).hint_text("Empty = safe defaults"));
                    ui.label("Review the file before starting: it may enable public read RPC or peer connections. This loopback admin endpoint remains explicit.");
                    if ui.button("Validate connection").clicked() {
                        self.node_info = None;
                        self.supply = None;
                        self.refresh_network();
                    }
                },
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label("Confirmation display target");
                ui.add(
                    egui::DragValue::new(&mut self.settings.confirmation_target).range(1..=10_000),
                );
            });
            ui.label("Rescan to apply this display policy. Higher-value payments may warrant more confirmations; no target guarantees finality.");
            ui.add_space(12.0);
            ui.label("Optional Reference Price · USD per GNR");
            ui.add(
                field(&mut self.settings.reference_price)
                    .hint_text("Empty = hidden; example 1.00")
                    .char_limit(24),
            );
            if !self.settings.reference_price.is_empty()
                && reference_value::format_usd("1000000000000", &self.settings.reference_price)
                    .is_err()
            {
                ui.label("Enter a positive plain decimal at most 1,000,000, or clear this field.");
            }
            ui.label("Display-only input, rounded down to cents. No price feed or transaction amount changes.");
            ui.horizontal(|ui| {
                ui.label("Appearance");
                for (light, label) in [(false, "Dark"), (true, "Light")] {
                    if ui
                        .add(egui::Button::new(label).selected(self.light == light))
                        .clicked()
                        && self.light != light
                    {
                        self.set_appearance(ui.ctx(), light);
                    }
                }
            });
            ui.label("Automatic wallet lock: 5 minutes of inactivity.");
            ui.label("Secrets are never passed on the command line. Only the update preference, appearance, remembered wallet paths and the network privacy mode (Network page) are persisted.");
        });
        ui.add_space(18.0);
        self.updates_card(ui);
        ui.add_space(18.0);
        self.about_card(ui);
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "EXPERIMENTAL SOFTWARE");
            ui.label(format!(
                "GENORYN Wallet {} · native Rust / egui",
                updates::CURRENT
            ));
            ui.label("GENORYN mainnet. This software has not been independently audited; only use amounts you can afford to lose.");
            ui.label("No price feed, telemetry, address-balance API, or automatic mining.");
        });
    }
}

#[cfg(test)]
mod odds_tests {
    use super::block_odds;

    #[test]
    fn expected_block_time_follows_difficulty_over_hashrate() {
        let text = block_odds(100.0, 40_794.0).unwrap();
        assert!(text.contains("about every 7 minutes"), "{text}");
        assert!(text.contains("14.7% of the network"), "{text}");
        assert!(
            block_odds(400.0, 100.0).unwrap().contains("0 seconds")
                || block_odds(400.0, 100.0).unwrap().contains("seconds")
        );
        assert!(block_odds(1.0, 1_000_000.0).unwrap().contains("days"));
        assert_eq!(block_odds(0.0, 10.0), None);
        assert_eq!(block_odds(f64::NAN, 10.0), None);
    }
}
