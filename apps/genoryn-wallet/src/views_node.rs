//! Native wallet views node; uses the shared backend.
use super::*;

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
            "Solo CPU mining. It starts only when you choose.",
        );
        ui.add_space(20.0);
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
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.unlocked
                            && self.node_info.is_some()
                            && self.miner.is_none()
                            && !self.process_busy,
                        primary(ui, "Start CPU mining").min_size(egui::vec2(200.0, 44.0)),
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
                for (key, label) in [
                    ("hashrate_hs", "Hashrate / second"),
                    ("hashes", "Hashes attempted"),
                    ("accepted_blocks", "Blocks accepted"),
                    ("rejected_blocks", "Blocks rejected"),
                    ("height", "Current height"),
                ] {
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
    pub(super) fn node_page(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Node", "Verify the network on your own machine.");
        ui.add_space(20.0);
        card(ui, |ui| {
            let title = if self.node.is_some() {
                "MANAGED NODE RUNNING"
            } else if self.node_info.is_some() {
                "EXTERNAL NODE CONNECTED"
            } else {
                "LOCAL NODE STOPPED / UNREACHABLE"
            };
            ui.label(RichText::new(title).size(22.0));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.node.is_none() && self.node_info.is_none() && !self.process_busy,
                        primary(ui, "Start local node").min_size(egui::vec2(180.0, 44.0)),
                    )
                    .clicked()
                {
                    self.start_process("node");
                }
                if ui
                    .add_enabled(
                        self.node.is_some() && !self.process_busy && self.miner.is_none(),
                        egui::Button::new("Stop owned node"),
                    )
                    .clicked()
                {
                    self.stop_process("node");
                }
                if ui.button("Check connection").clicked() {
                    self.refresh_network();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.node.is_some() && self.miner.is_none() && !self.process_busy,
                        egui::Button::new("Restart owned node"),
                    )
                    .clicked()
                {
                    self.restart_owned_node();
                }
                if ui
                    .add_enabled(!self.log_busy, egui::Button::new("Open recent logs"))
                    .clicked()
                {
                    self.load_node_log();
                }
                if ui.button("Advanced configuration").clicked() {
                    self.page = 7;
                }
            });
            ui.add_space(14.0);
            ui.label("Only processes started by this wallet can be stopped here. Stop mining before stopping your node.");
            ui.label(
                RichText::new(format!(
                    "Logs: {}",
                    PathBuf::from(&self.settings.data_dir)
                        .join("genorynd.log")
                        .display()
                ))
                .small()
                .color(GRAY),
            );
        });
        ui.add_space(18.0);
        card(ui, |ui| {
            small_title(ui, "NODE DIAGNOSTICS");
            if let Some(node) = &self.node_info {
                diagnostic(ui, "Height", node.info.height);
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
                ui.label(RichText::new("Sync percentage is unknown without an independently verified target height. Peer claims do not establish global sync.").small().color(GRAY));
            } else {
                ui.label("No verified response from the configured node.");
                if let Some(error) = &self.network_error {
                    ui.label(RichText::new(error).color(GRAY));
                }
            }
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
