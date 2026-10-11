//! Wallet → Privacy & Security: what this computer can actually observe about
//! its privacy and security, check by check.
//!
//! Every check has one of four results and says what it is based on. A
//! setting the user chose is reported as a setting, never as verified; a
//! backup the user confirmed is reported as their statement; what cannot be
//! observed from inside this application is "Not tested". There is no score:
//! unrelated properties do not add up to one number.
use super::*;
use genoryn_wallet_core::hygiene;

/// Test results, plus two informational kinds that are never verification:
/// a setting the user chose and a statement the user made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Level {
    Verified,
    Warning,
    Failed,
    Unknown,
    Setting,
    Statement,
}
impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified",
            Self::Warning => "Warning",
            Self::Failed => "Failed",
            Self::Unknown => "Not tested",
            Self::Setting => "Your setting",
            Self::Statement => "Your statement",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Check {
    pub group: &'static str,
    pub name: &'static str,
    pub level: Level,
    pub finding: String,
    pub basis: &'static str,
    pub action: Option<&'static str>,
}

/// The results of the controlled checks, made in the background.
#[derive(Clone, Debug, Default)]
pub(super) struct Diagnostics {
    pub at: u64,
    pub proxy_fails_closed: Option<Result<(), String>>,
    pub local_resolution_refused: bool,
    pub remote_rpc_refused_in_tor_only: bool,
    pub storage: Option<hygiene::StorageReport>,
    pub wallet_file_sealed: Option<bool>,
}

#[derive(Default)]
pub(super) struct SecurityUi {
    pub diagnostics: Option<Diagnostics>,
    pub running: bool,
}

/// A loopback port nothing listens on: connecting to it fails at once.
fn closed_local_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|listener| listener.local_addr().ok())
        .map(|address| address.port())
}

/// Controlled checks of the wallet's own network rules, all on this computer:
/// a request through a proxy that cannot be reached must fail instead of
/// going out directly, and a proxy that would resolve names locally is refused.
pub(super) fn run_diagnostics(wallet_path: &str, data_dir: &str) -> Diagnostics {
    let proxy_fails_closed = closed_local_port().map(|port| {
        let proxy = format!("socks5h://127.0.0.1:{port}");
        (|| -> Result<()> {
            let client = network_privacy::http_client(Some(&proxy))?
                .timeout(Duration::from_secs(5))
                .build()?;
            let attempt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(async { client.get("https://genoryn.invalid/").send().await });
            ensure!(
                attempt.is_err(),
                "a request succeeded although its proxy was unreachable"
            );
            Ok(())
        })()
        .map_err(|error| error.to_string())
    });
    let local_resolution_refused =
        network_privacy::http_client(Some("socks5://127.0.0.1:9050")).is_err();
    let previous = network_privacy::active();
    network_privacy::set_active(network_privacy::Mode::TorOnly);
    let remote_rpc_refused_in_tor_only = network_privacy::guard_rpc("https://node.example.org")
        .is_err()
        && network_privacy::guard_rpc("http://127.0.0.1:29711").is_ok();
    network_privacy::set_active(previous);
    let wallet = PathBuf::from(wallet_path);
    let mut folders = vec![PathBuf::from(data_dir)];
    if let Some(parent) = wallet.parent().filter(|p| !p.as_os_str().is_empty())
        && !folders
            .iter()
            .any(|f| sessions::same_path(&f.display().to_string(), &parent.display().to_string()))
    {
        folders.push(parent.to_owned());
    }
    let storage = Some(hygiene::check_storage(&folders));
    let wallet_file_sealed = std::fs::read(&wallet).ok().map(|bytes| {
        bytes.len() <= 16_384
            && serde_json::from_slice::<Value>(&bytes).is_ok_and(|envelope| {
                envelope["version"] == 1
                    && envelope["ciphertext"]
                        .as_array()
                        .is_some_and(|c| c.len() >= 48)
                    && envelope["salt"].as_array().is_some_and(|s| s.len() == 16)
            })
            && !hygiene::contains_recovery_phrase(&String::from_utf8_lossy(&bytes))
    });
    Diagnostics {
        at: genoryn_core::chain::unix_time(),
        proxy_fails_closed,
        local_resolution_refused,
        remote_rpc_refused_in_tor_only,
        storage,
        wallet_file_sealed,
    }
}

impl WalletApp {
    pub(super) fn start_security_check(&mut self) {
        if self.security.running {
            return;
        }
        self.security.running = true;
        let (tx, wallet, data) = (
            self.tx.clone(),
            self.settings.wallet_path.clone(),
            self.settings.data_dir.clone(),
        );
        thread::spawn(move || {
            let _ = tx.send(Event::Security(Box::new(run_diagnostics(&wallet, &data))));
        });
    }

    /// Every check, from what this wallet and its node report now.
    pub(super) fn security_checks(&self) -> Vec<Check> {
        let mut checks = Vec::new();
        let configured = network_privacy::Mode::parse(&self.preferences.network_mode)
            .unwrap_or(network_privacy::Mode::Direct);
        let privacy = self.network_privacy.as_ref();
        // What the node actually runs decides the network checks; the setting
        // only says what a node started by this wallet is asked to run.
        let observed = privacy
            .and_then(|status| status["mode"].as_str())
            .and_then(network_privacy::Mode::parse);
        let mode = observed.unwrap_or(configured);
        let quick = self.preferences.light_wallet;
        checks.push(Check {
            group: "Payments",
            name: "On-chain shielded payments",
            level: Level::Verified,
            finding: "Every transfer on GENORYN is a shielded Orchard (Halo 2) transaction: amounts and addresses are encrypted. Consensus rejects unshielded transfers.".into(),
            basis: "consensus rule enforced by every node and by this wallet's validation",
            action: None,
        });
        checks.push(Check {
            group: "Network",
            name: "Configured network privacy mode",
            level: match configured {
                network_privacy::Mode::TorOnly => Level::Setting,
                _ => Level::Warning,
            },
            finding: format!(
                "{} is selected. {}{}",
                configured.label(),
                configured.warning().unwrap_or("All of this wallet's node connections are configured to use Tor."),
                match observed {
                    Some(running) if running != configured => format!(
                        " The node you are connected to actually runs {} (it was started with other settings, or elsewhere); the checks below follow what it runs.",
                        running.label()
                    ),
                    _ => String::new(),
                }
            ),
            basis: "your setting (what the node is asked to do; the checks below show what it does)",
            action: (mode != network_privacy::Mode::TorOnly)
                .then_some("Choose Tor only on the Network page to hide this computer's IP address from peers."),
        });
        let tor = privacy.map(|status| &status["tor"]);
        checks.push(match (mode, tor) {
            (network_privacy::Mode::Direct, _) => Check {
                group: "Network",
                name: "Tor bootstrap",
                level: Level::Unknown,
                finding: "Tor is not used in Direct mode.".into(),
                basis: "the node's mode",
                action: None,
            },
            (_, Some(tor)) if tor["ready"] == true => Check {
                group: "Network",
                name: "Tor bootstrap",
                level: Level::Verified,
                finding: "The node's Tor client reports a complete bootstrap.".into(),
                basis: "reported by your own node (/v1/network)",
                action: None,
            },
            (_, Some(_)) => Check {
                group: "Network",
                name: "Tor bootstrap",
                level: Level::Failed,
                finding: "Tor is not ready. In Tor-only mode the node pauses networking instead of connecting directly.".into(),
                basis: "reported by your own node (/v1/network)",
                action: Some("Wait a minute, or use Retry Tor on the Network page."),
            },
            (_, None) => Check {
                group: "Network",
                name: "Tor bootstrap",
                level: Level::Unknown,
                finding: "No local node reports its Tor status (Quick mode starts Tor only while syncing).".into(),
                basis: "no observation available",
                action: None,
            },
        });
        let connections = privacy.map(|status| &status["connections"]);
        let count = |key: &str| connections.and_then(|c| c[key].as_u64()).unwrap_or(0);
        let transport = privacy.map(|status| &status["transport"]);
        let direct_attempts = transport.and_then(|t| t["direct_attempts"].as_u64());
        match connections {
            Some(_) => {
                let tor_peers = count("tor_outbound") + count("onion_inbound");
                let clearnet = count("clearnet_inbound") + count("clearnet_outbound");
                checks.push(Check {
                    group: "Network",
                    name: "Peers reached through Tor",
                    level: if tor_peers > 0 { Level::Verified } else if mode == network_privacy::Mode::Direct { Level::Unknown } else { Level::Warning },
                    finding: format!("{tor_peers} of this node's peer connections use Tor ({} outbound, {} inbound through its onion service).", count("tor_outbound"), count("onion_inbound")),
                    basis: "connection counts reported by your own node",
                    action: (tor_peers == 0 && mode != network_privacy::Mode::Direct).then_some("Add an onion peer on the Network page, or wait for Tor peers."),
                });
                let tor_only = mode == network_privacy::Mode::TorOnly;
                checks.push(Check {
                    group: "Network",
                    name: "Direct (clearnet) peer connections",
                    level: match (tor_only, clearnet, direct_attempts) {
                        (true, 0, Some(0)) => Level::Verified,
                        (true, 0, _) => Level::Verified,
                        (true, _, _) => Level::Failed,
                        (false, 0, _) => Level::Verified,
                        (false, _, _) => Level::Warning,
                    },
                    finding: format!(
                        "{clearnet} direct connection(s) now; {} direct connection attempt(s) since the node started.",
                        direct_attempts.map_or("an unknown number of".into(), |n| n.to_string())
                    ),
                    basis: "your own node's live connections and transport counters",
                    action: (!tor_only && clearnet > 0).then_some("Direct peers see this computer's IP address. Use Tor only to avoid this."),
                });
            }
            None => checks.push(Check {
                group: "Network",
                name: "Peer connections",
                level: Level::Unknown,
                finding: if quick { "Quick mode has no peers of its own: it reaches remote nodes only through Tor while it syncs.".into() } else { "No local node reports its connections.".into() },
                basis: "no observation available",
                action: None,
            }),
        }
        match &self.security.diagnostics {
            Some(diagnostics) => {
                checks.push(Check {
                    group: "Network",
                    name: "Proxy failure fails closed",
                    level: match &diagnostics.proxy_fails_closed {
                        Some(Ok(())) => Level::Verified,
                        Some(Err(_)) => Level::Failed,
                        None => Level::Unknown,
                    },
                    finding: match &diagnostics.proxy_fails_closed {
                        Some(Ok(())) => "A test request through an unreachable Tor proxy failed; it did not go out directly.".into(),
                        Some(Err(error)) => error.clone(),
                        None => "The test could not run.".into(),
                    },
                    basis: "controlled test on this computer (loopback only, nothing sent to the internet)",
                    action: None,
                });
                checks.push(Check {
                    group: "Network",
                    name: "Names resolved by Tor, not local DNS",
                    level: if diagnostics.local_resolution_refused { Level::Verified } else { Level::Failed },
                    finding: "The wallet refuses proxies that would look names up on this computer (socks5); it only uses socks5h, where Tor resolves names. Onion names are never given to the system resolver.".into(),
                    basis: "controlled test of the wallet's own HTTP rules; the operating system's DNS is not observed",
                    action: None,
                });
                checks.push(Check {
                    group: "Network",
                    name: "Tor-only keeps wallet RPC local",
                    level: if diagnostics.remote_rpc_refused_in_tor_only { Level::Verified } else { Level::Failed },
                    finding: "In Tor-only mode the wallet refuses to contact a remote RPC endpoint directly and talks only to your own local node.".into(),
                    basis: "controlled test of the wallet's own rules",
                    action: None,
                });
            }
            None => checks.push(Check {
                group: "Network",
                name: "Controlled network tests",
                level: Level::Unknown,
                finding: "Not run yet.".into(),
                basis: "press Run security check",
                action: Some("Run the security check."),
            }),
        }
        checks.push(Check {
            group: "Synchronization",
            name: "Synchronization source",
            level: if quick { Level::Warning } else if self.node_info.is_some() { Level::Verified } else { Level::Unknown },
            finding: if quick {
                format!("Quick mode: blocks come from {} remote node(s) over Tor and every block is validated on this computer.", self.preferences.light_sources.len())
            } else if self.node.is_some() {
                "Full mode with the node this wallet started: it validates every block itself.".into()
            } else if self.node_info.is_some() {
                format!("Full mode with the node at {}.", self.settings.rpc_url)
            } else {
                "No node is connected.".into()
            },
            basis: "wallet mode and the connected node",
            action: quick.then_some("Run your own full node (My Full Node) to stop depending on remote sources for new blocks."),
        });
        checks.push(Check {
            group: "Synchronization",
            name: "Remote node trust",
            level: if quick { Level::Warning } else { Level::Verified },
            finding: if quick { network_privacy::LIGHT_TRUST.into() } else { "Your own node validates every block; no remote node is trusted for balances or history.".into() },
            basis: "how this wallet validates (consensus checks run locally)",
            action: None,
        });
        checks.push(Check {
            group: "Wallet",
            name: "Wallet lock",
            level: if self.unlocked {
                Level::Warning
            } else {
                Level::Verified
            },
            finding: if self.unlocked {
                format!(
                    "{} wallet(s) unlocked now. Keys stay in this process while unlocked.",
                    self.open_wallet_count().max(1)
                )
            } else {
                "All wallets are locked.".into()
            },
            basis: "this wallet's state",
            action: self
                .unlocked
                .then_some("Lock the wallet when you step away."),
        });
        checks.push(Check {
            group: "Wallet",
            name: "Inactivity lock",
            level: Level::Verified,
            finding: "Unlocked wallets lock after 5 minutes without input; the password and recovery words are wiped from memory.".into(),
            basis: "built into this wallet",
            action: None,
        });
        if let Some(diagnostics) = &self.security.diagnostics {
            checks.push(Check {
                group: "Wallet",
                name: "Wallet file encryption",
                level: match diagnostics.wallet_file_sealed {
                    Some(true) => Level::Verified,
                    Some(false) => Level::Failed,
                    None => Level::Unknown,
                },
                finding: match diagnostics.wallet_file_sealed {
                    Some(true) => "The open wallet file is an encrypted envelope (Argon2id key, XChaCha20-Poly1305) and holds no readable recovery words.".into(),
                    Some(false) => "The wallet file is not in the expected encrypted format.".into(),
                    None => "The wallet file could not be read.".into(),
                },
                basis: "format check of the wallet file on disk (nothing is decrypted)",
                action: None,
            });
            if let Some(storage) = &diagnostics.storage {
                checks.push(Check {
                    group: "Wallet",
                    name: "GENORYN storage hygiene",
                    level: if !storage.findings.is_empty() { Level::Failed } else if storage.truncated { Level::Warning } else { Level::Verified },
                    finding: if storage.findings.is_empty() {
                        format!("{} file(s) in GENORYN's own folders checked; none looks like a plain-text recovery phrase.", storage.files_checked)
                    } else {
                        storage.findings.iter().map(|f| format!("{}: {}", f.file.display(), f.issue)).collect::<Vec<_>>().join("\n")
                    },
                    basis: "GENORYN's data and wallet folders only; the rest of this computer is never searched",
                    action: (!storage.findings.is_empty()).then_some("Move the listed file off this computer or destroy it after making a paper backup."),
                });
            }
        }
        let confirmed = self
            .preferences
            .backup_confirmed
            .get(&sessions::path_key(&self.settings.wallet_path))
            .copied();
        checks.push(Check {
            group: "Wallet",
            name: "Recovery phrase backup",
            level: if confirmed.is_some() { Level::Statement } else { Level::Warning },
            finding: match confirmed {
                Some(at) => format!("You confirmed a backup of this wallet's 24 words on {}. GENORYN cannot see where or how it is kept.", proof_formats::utc_text(at)),
                None => "No backup confirmed for this wallet.".into(),
            },
            basis: "your own statement (not independently verified)",
            action: confirmed.is_none().then_some("Write the 24 words on paper, keep them offline, then confirm below."),
        });
        checks.push(Check {
            group: "Software",
            name: "Version and updates",
            level: match &self.update_status {
                updates::UpdateStatus::UpToDate => Level::Verified,
                updates::UpdateStatus::Available(_) => Level::Warning,
                updates::UpdateStatus::Failed(_) => Level::Failed,
                _ => Level::Unknown,
            },
            finding: match &self.update_status {
                updates::UpdateStatus::UpToDate => format!(
                    "GENORYN {} is the latest published release.",
                    updates::CURRENT
                ),
                updates::UpdateStatus::Available(release) => format!(
                    "GENORYN {} is available; you run {}.",
                    release.version,
                    updates::CURRENT
                ),
                updates::UpdateStatus::Failed(error) => format!("The update check failed: {error}"),
                _ => format!(
                    "Running GENORYN {}; updates not checked yet.",
                    updates::CURRENT
                ),
            },
            basis: "the public GitHub release record (through Tor in Tor-only mode)",
            action: matches!(self.update_status, updates::UpdateStatus::Available(_))
                .then_some("Download the update from the official release page and verify it."),
        });
        checks.push(Check {
            group: "Software",
            name: "Release authenticity",
            level: Level::Warning,
            finding: "Releases publish SHA-256 checksums beside the files. A checksum from the same place as the download detects damage, not a replaced release: GENORYN packages are not yet signed with an independent publisher key.".into(),
            basis: "how GENORYN is published today",
            action: Some("Compare checksums from more than one source (GitHub release, genoryn.io over Tor), or build from source."),
        });
        checks
    }

    pub(super) fn security_page(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Privacy & Security",
            "What this computer can actually observe. No promises it cannot check.",
        );
        ui.add_space(18.0);
        let checks = self.security_checks();
        let count = |level| checks.iter().filter(|c| c.level == level).count();
        card(ui, |ui| {
            small_title(ui, "SECURITY CHECK");
            ui.horizontal_wrapped(|ui| {
                for (level, label) in [
                    (Level::Verified, "verified"),
                    (Level::Warning, "need attention"),
                    (Level::Failed, "failed"),
                    (Level::Unknown, "not tested"),
                    (Level::Setting, "settings"),
                    (Level::Statement, "your statements"),
                ] {
                    ui.label(RichText::new(format!("{} {label}", count(level))).strong());
                    ui.add_space(14.0);
                }
            });
            ui.label(
                RichText::new(match &self.security.diagnostics {
                    Some(d) => format!(
                        "Last controlled test {} · wallet {} · node {}",
                        proof_formats::utc_text(d.at),
                        updates::CURRENT,
                        self.node_info
                            .as_ref()
                            .map(|n| format!("height {}", n.info.height))
                            .unwrap_or_else(|| "not connected".into())
                    ),
                    None => "The controlled tests have not run in this session.".into(),
                })
                .small()
                .color(GRAY),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if layout_probe::tag(
                    "security:run",
                    ui.add_enabled(
                        !self.security.running,
                        primary(ui, "Run security check").min_size(egui::vec2(200.0, 44.0)),
                    ),
                )
                .clicked()
                {
                    self.start_security_check();
                    self.refresh_network();
                }
                if self.security.running {
                    motion::loader(ui, 14.0);
                    ui.label("Testing…");
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new("Nothing is uploaded and no privacy-check website is contacted. The tests use only this computer and your own node.")
                    .small()
                    .color(GRAY),
            );
        });
        for group in [
            "Network",
            "Synchronization",
            "Wallet",
            "Payments",
            "Software",
        ] {
            ui.add_space(14.0);
            card(ui, |ui| {
                small_title(ui, group);
                for check in checks.iter().filter(|c| c.group == group) {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(96.0, 20.0), egui::Sense::hover());
                        let strong = ui.visuals().strong_text_color();
                        let ink = match check.level {
                            Level::Failed => ERROR,
                            Level::Unknown | Level::Setting | Level::Statement => GRAY,
                            _ => strong,
                        };
                        ui.painter().rect_stroke(
                            rect,
                            6.0,
                            Stroke::new(1.0, ink),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            check.level.label(),
                            egui::FontId::proportional(11.5),
                            ink,
                        );
                        ui.label(RichText::new(check.name).strong());
                    });
                    ui.add(egui::Label::new(check.finding.as_str()).wrap());
                    ui.label(
                        RichText::new(format!("Based on: {}", check.basis))
                            .small()
                            .color(GRAY),
                    );
                    if let Some(action) = check.action {
                        ui.label(RichText::new(format!("Recommended: {action}")).small());
                    }
                    ui.add_space(10.0);
                }
                if group == "Wallet" {
                    self.backup_guidance(ui);
                }
            });
        }
        ui.add_space(14.0);
        ui.label(RichText::new(network_privacy::LAYERS).small().color(GRAY));
    }

    fn backup_guidance(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new("Backup checklist").strong());
        for line in [
            "Write the 24 words on paper (or metal), in order, and check every word.",
            "Keep them offline: no photo, screenshot, cloud note, e-mail or chat message.",
            "Store them where only you (or someone you trust) can reach them.",
            "Anyone with the 24 words can spend this wallet. GENORYN never asks for them online.",
        ] {
            ui.label(format!("·  {line}"));
        }
        ui.add_space(6.0);
        let key = sessions::path_key(&self.settings.wallet_path);
        let mut confirmed = self.preferences.backup_confirmed.contains_key(&key);
        if ui
            .add_enabled(
                self.unlocked,
                egui::Checkbox::new(
                    &mut confirmed,
                    "I have a safe offline backup of this wallet's recovery phrase",
                ),
            )
            .changed()
        {
            if confirmed {
                self.preferences
                    .backup_confirmed
                    .insert(key, genoryn_core::chain::unix_time());
            } else {
                self.preferences.backup_confirmed.remove(&key);
            }
            self.save_preferences();
        }
        ui.label(
            RichText::new("This records your statement only. The phrase is never shown, exported or checked here.")
                .small()
                .color(GRAY),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controlled_checks_fail_closed_and_never_overclaim() {
        let directory =
            std::env::temp_dir().join(format!("genoryn-security-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let wallet = directory.join("w.gnr");
        let diagnostics = run_diagnostics(
            &wallet.display().to_string(),
            &directory.display().to_string(),
        );
        assert_eq!(diagnostics.proxy_fails_closed, Some(Ok(())));
        assert!(diagnostics.local_resolution_refused);
        assert!(diagnostics.remote_rpc_refused_in_tor_only);
        assert_eq!(diagnostics.wallet_file_sealed, None, "no wallet file there");
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.preferences.network_mode = "direct".into();
        let checks = app.security_checks();
        let find = |name: &str| checks.iter().find(|c| c.name == name).unwrap().level;
        assert_eq!(find("Configured network privacy mode"), Level::Warning);
        assert_eq!(find("Recovery phrase backup"), Level::Warning);
        assert_eq!(
            find("Release authenticity"),
            Level::Warning,
            "a checksum is not a signature"
        );
        assert_eq!(find("Controlled network tests"), Level::Unknown);
        app.preferences.network_mode = "tor-only".into();
        app.network_privacy = Some(
            json!({"mode": "tor-only", "tor": {"ready": false}, "connections": {"clearnet_outbound": 1, "tor_outbound": 0, "onion_inbound": 0, "clearnet_inbound": 0}, "transport": {"direct_attempts": 3}}),
        );
        let checks = app.security_checks();
        let find = |name: &str| checks.iter().find(|c| c.name == name).unwrap().level;
        assert_eq!(find("Tor bootstrap"), Level::Failed);
        assert_eq!(find("Direct (clearnet) peer connections"), Level::Failed);
        assert!(!checks.iter().any(|c| c.finding.contains("100%")));
        app.security.diagnostics = Some(diagnostics);
        app.preferences
            .backup_confirmed
            .insert(sessions::path_key(&app.settings.wallet_path), 1);
        let checks = app.security_checks();
        let backup = checks
            .iter()
            .find(|c| c.name == "Recovery phrase backup")
            .unwrap();
        assert_eq!(
            backup.level,
            Level::Statement,
            "a statement is never verification"
        );
        assert!(backup.basis.contains("not independently verified"));
        let mode = checks
            .iter()
            .find(|c| c.name == "Configured network privacy mode")
            .unwrap();
        assert_eq!(
            mode.level,
            Level::Setting,
            "a setting is never verification"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
