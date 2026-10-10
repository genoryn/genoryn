//! Wallet state shown besides balance and history: where the balance was
//! verified and how current it is, mining rewards with their maturity, payments
//! waiting in the durable outbox, the Explorer link and the About section.
use super::*;

pub(super) const EXPLORER_URL: &str = "https://genoryn.io/";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RewardFilter {
    #[default]
    All,
    JustMined,
    Maturing,
    Mature,
    Replaced,
}
impl RewardFilter {
    pub const ALL: [RewardFilter; 5] = [
        Self::All,
        Self::JustMined,
        Self::Maturing,
        Self::Mature,
        Self::Replaced,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::JustMined => "Just mined",
            Self::Maturing => "Maturing",
            Self::Mature => "Mature",
            Self::Replaced => "Replaced",
        }
    }
    fn matches(self, status: &str) -> bool {
        match self {
            Self::All => true,
            Self::JustMined => status == "JustMined",
            // A reward waiting for the chain to reach its height again is still
            // maturing as far as spending is concerned: not available.
            Self::Maturing => matches!(status, "Maturing" | "Unconfirmed"),
            Self::Mature => status == "Mature",
            Self::Replaced => status == "Replaced",
        }
    }
}

/// Sums `amount_atomic` of rewards whose status is in `statuses`.
pub(super) fn reward_total(rewards: &[Value], statuses: &[&str]) -> u128 {
    rewards
        .iter()
        .filter(|reward| statuses.contains(&reward["status"].as_str().unwrap_or_default()))
        .filter_map(|reward| reward["amount_atomic"].as_str()?.parse::<u128>().ok())
        .sum()
}

impl WalletApp {
    pub(super) fn rewards(&self) -> Vec<Value> {
        self.sync_detail
            .as_ref()
            .and_then(|detail| detail["mined"].as_array().cloned())
            .unwrap_or_default()
    }
    fn outbox_entries(&self) -> Vec<Value> {
        self.sync_detail
            .as_ref()
            .and_then(|detail| detail["outbox"].as_array().cloned())
            .unwrap_or_default()
    }
    pub(super) fn balance_is_stale(&self) -> bool {
        self.sync_detail
            .as_ref()
            .is_some_and(|detail| detail["stale"] == true)
    }
    /// One line saying how current the balance is and who verified it. An
    /// unknown balance is never presented as zero, a stale one never as current.
    pub(super) fn balance_state(&self, ui: &mut egui::Ui) {
        let text = match (self.wallet_height, &self.sync_detail) {
            (None, _) if self.scan_busy.is_some() => {
                "Synchronizing · balance not known yet".to_owned()
            }
            (None, _) => "Not scanned yet · balance unknown".to_owned(),
            (Some(height), Some(detail)) if detail["stale"] == true => format!(
                "Last verified at block {height} · remote nodes unreachable, the balance may be out of date{}",
                detail["offline_reason"]
                    .as_str()
                    .map(|reason| format!(" ({})", reason.chars().take(90).collect::<String>()))
                    .unwrap_or_default()
            ),
            (Some(height), detail) => {
                let behind = self
                    .node_info
                    .as_ref()
                    .is_some_and(|node| node.info.height > height);
                let who = detail
                    .as_ref()
                    .and_then(|d| d["verified_by"].as_str())
                    .unwrap_or("your node");
                if self.scan_busy.is_some() || behind {
                    format!("Last verified at block {height} by {who} · synchronizing")
                } else {
                    format!("Fully synced at block {height} · verified by {who}")
                }
            }
        };
        let color = if self.balance_is_stale() { ERROR } else { GRAY };
        ui.label(RichText::new(text).small().color(color));
    }
    /// Recent rewards and their maturity on the Overview page.
    pub(super) fn mining_overview(&self, ui: &mut egui::Ui) {
        let rewards = self.rewards();
        if rewards.is_empty() {
            return;
        }
        card(ui, |ui| {
            small_title(ui, "MINING REWARDS");
            let latest = rewards
                .iter()
                .find(|reward| reward["status"] != "Replaced" && reward["status"] != "Unconfirmed");
            ui.label(match latest {
                Some(reward) => format!(
                    "Latest canonical block: {} · {}",
                    reward["height"],
                    reward["status_label"].as_str().unwrap_or_default()
                ),
                None => "No reward on the current chain.".into(),
            });
            ui.add_space(6.0);
            for (label, statuses) in [
                ("Immature rewards", &["JustMined", "Maturing"][..]),
                ("Mature rewards", &["Mature"][..]),
                ("Verification pending", &["Unconfirmed"][..]),
                ("Replaced (history only)", &["Replaced"][..]),
            ] {
                let total = reward_total(&rewards, statuses);
                let count = rewards
                    .iter()
                    .filter(|r| statuses.contains(&r["status"].as_str().unwrap_or_default()))
                    .count();
                if count == 0 && label != "Immature rewards" && label != "Mature rewards" {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [200.0, 20.0],
                        egui::Label::new(RichText::new(format!("{label} ({count})")).color(GRAY)),
                    );
                    gnr_amount(ui, Some(&total.to_string()), 14.0, None);
                });
            }
            ui.label(
                RichText::new("Mature means spendable under consensus, not unspent. Replaced rewards were in blocks that a heavier chain displaced; they are never counted in any balance.")
                    .small()
                    .color(GRAY),
            );
            ui.add_space(6.0);
            small_title(ui, "RECENT");
            for reward in rewards.iter().take(5) {
                reward_line(ui, reward);
            }
        });
    }
    /// The Mined tab: every reward the wallet has seen, replaced ones included.
    pub(super) fn mined_rewards(&mut self, ui: &mut egui::Ui) {
        let rewards = self.rewards();
        let total_canonical = reward_total(&rewards, &["JustMined", "Maturing", "Mature"]);
        let canonical = rewards
            .iter()
            .filter(|r| {
                matches!(
                    r["status"].as_str(),
                    Some("JustMined" | "Maturing" | "Mature")
                )
            })
            .count();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    small_title(ui, "MINED ON THE CURRENT CHAIN");
                    gnr_amount(ui, Some(&total_canonical.to_string()), 26.0, None);
                });
                ui.add_space(40.0);
                ui.vertical(|ui| {
                    small_title(ui, "REWARDED BLOCKS");
                    ui.label(RichText::new(canonical.to_string()).size(26.0));
                });
                ui.add_space(40.0);
                ui.vertical(|ui| {
                    small_title(ui, "SPENDABLE NOW");
                    gnr_amount(
                        ui,
                        Some(&reward_total(&rewards, &["Mature"]).to_string()),
                        26.0,
                        None,
                    );
                });
            });
            ui.label(
                RichText::new("Historically mined is not the same as available: immature and replaced rewards cannot be spent.")
                    .small()
                    .color(GRAY),
            );
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            for filter in RewardFilter::ALL {
                let count = rewards
                    .iter()
                    .filter(|r| filter.matches(r["status"].as_str().unwrap_or_default()))
                    .count();
                if ui
                    .add(
                        egui::Button::new(format!("{}  {count}", filter.label()))
                            .selected(self.tx_view.rewards == filter),
                    )
                    .clicked()
                {
                    self.tx_view.rewards = filter;
                }
            }
        });
        ui.add_space(10.0);
        let shown: Vec<&Value> = rewards
            .iter()
            .filter(|r| {
                self.tx_view
                    .rewards
                    .matches(r["status"].as_str().unwrap_or_default())
            })
            .collect();
        if shown.is_empty() {
            card(ui, |ui| {
                ui.label(
                    RichText::new(if rewards.is_empty() {
                        "No mining rewards for this wallet in the scanned chain."
                    } else {
                        "No rewards match this filter."
                    })
                    .color(GRAY),
                );
            });
            return;
        }
        for reward in shown.iter().take(self.tx_view.limit.max(50)) {
            card(ui, |ui| reward_row(ui, reward));
            ui.add_space(6.0);
        }
    }
    /// Payments signed on this computer whose outcome the chain has not settled,
    /// and recently settled ones.
    pub(super) fn outbox_card(&self, ui: &mut egui::Ui) {
        let entries = self.outbox_entries();
        if entries.is_empty() {
            return;
        }
        card(ui, |ui| {
            small_title(ui, "OUTGOING PAYMENTS");
            ui.label(
                RichText::new("Stored on this computer before sending and retried automatically with the same signed bytes. A node's acknowledgement is not a confirmation; only a block is.")
                    .small()
                    .color(GRAY),
            );
            ui.add_space(6.0);
            for entry in &entries {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(entry["status_label"].as_str().unwrap_or("Pending")).strong(),
                    );
                    gnr_amount(ui, entry["amount_atomic"].as_str(), 14.0, None);
                    ui.label(
                        RichText::new(format!(
                            "to {}",
                            short_text(entry["recipient"].as_str().unwrap_or_default(), 28)
                        ))
                        .color(GRAY),
                    );
                    let txid = entry["txid"].as_str().unwrap_or_default();
                    ui.label(RichText::new(short_text(txid, 16)).monospace().small());
                    if ui.small_button("Copy ID").clicked() {
                        ui.ctx().copy_text(txid.to_owned());
                    }
                });
                let detail = match entry["status"].as_str() {
                    Some("Confirmed") => {
                        format!("Included in block {}.", entry["confirmed_height"])
                    }
                    Some("Expired") => {
                        "Not included before its expiry; the coins are available again.".into()
                    }
                    Some("Conflicted") => "Its coins were spent by another transaction.".into(),
                    Some("Reorged") => {
                        "A reorganization removed its block; it is being sent again.".into()
                    }
                    Some("Submitted") => format!(
                        "Acknowledged by a node; waiting for a block (expires after block {}).",
                        entry["expires"]
                    ),
                    _ => format!(
                        "Not acknowledged yet; {} attempt(s){}",
                        entry["attempts"],
                        entry["last_error"]
                            .as_str()
                            .map(|e| format!(" · last error: {}", short_text(e, 120)))
                            .unwrap_or_default()
                    ),
                };
                ui.label(RichText::new(detail).small().color(GRAY));
                ui.add_space(6.0);
            }
        });
        ui.add_space(12.0);
    }
    /// The sidebar link to the public explorer; opens only when clicked.
    pub(super) fn explorer_link(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(162.0, 46.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Link, true, "GENORYN Explorer")
        });
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(EXPLORER_URL);
        let dark = ui.visuals().dark_mode;
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .rect_filled(rect, 7.0, Color32::from_gray(if dark { 19 } else { 233 }));
        }
        let ink = if hovered {
            if dark { Color32::WHITE } else { Color32::BLACK }
        } else {
            Color32::from_gray(if dark { 165 } else { 85 })
        };
        let painter = ui.painter();
        painter.text(
            egui::pos2(rect.left() + 12.0, rect.top() + 15.0),
            egui::Align2::LEFT_CENTER,
            "GENORYN Explorer",
            egui::FontId::proportional(14.0),
            ink,
        );
        painter.text(
            egui::pos2(rect.right() - 10.0, rect.top() + 15.0),
            egui::Align2::RIGHT_CENTER,
            "↗",
            egui::FontId::proportional(13.0),
            ink,
        );
        painter.text(
            egui::pos2(rect.left() + 12.0, rect.top() + 33.0),
            egui::Align2::LEFT_CENTER,
            "Blocks · Transactions · Network",
            egui::FontId::proportional(10.5),
            GRAY,
        );
        if response.clicked() {
            if network_privacy::active() == network_privacy::Mode::TorOnly {
                self.explorer_confirm = true;
            } else {
                ui.ctx().open_url(egui::OpenUrl::new_tab(EXPLORER_URL));
            }
        }
    }
    /// In Tor-only mode the browser is not routed through GENORYN's Tor.
    pub(super) fn explorer_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.explorer_confirm {
            return;
        }
        egui::Window::new("Open GENORYN Explorer")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.label("This wallet is in Tor-only mode, but your web browser does not use GENORYN's Tor. Opening the Explorer connects to genoryn.io directly and reveals this computer's IP address to that website.");
                ui.add_space(8.0);
                ui.label(RichText::new("Use Tor Browser instead if that matters to you.").color(GRAY));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Open in my browser").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(EXPLORER_URL));
                        self.explorer_confirm = false;
                    }
                    if ui.button("Copy address").clicked() {
                        ui.ctx().copy_text(EXPLORER_URL.to_owned());
                        self.explorer_confirm = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.explorer_confirm = false;
                    }
                });
            });
    }
    /// Version, build and network identity.
    pub(super) fn about_card(&self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            small_title(ui, "ABOUT");
            diagnostic(
                ui,
                "Application",
                format!("GENORYN Wallet {}", updates::CURRENT),
            );
            diagnostic(
                ui,
                "Source commit",
                option_env!("GENORYN_COMMIT").unwrap_or("development build"),
            );
            diagnostic(ui, "Network", NETWORK.name());
            diagnostic(ui, "Chain ID", NETWORK.chain_id());
            diagnostic(ui, "Genesis", hex::encode(NETWORK.genesis()));
            diagnostic(
                ui,
                "Wallet mode",
                if self.preferences.light_wallet {
                    "Quick (remote nodes over Tor, validated locally)"
                } else {
                    "Full (your own node)"
                },
            );
            ui.label(
                RichText::new("Experimental software. Not independently audited.")
                    .small()
                    .color(GRAY),
            );
        });
    }
}

fn short_text(text: &str, keep: usize) -> String {
    if text.chars().count() <= keep {
        return text.to_owned();
    }
    let head: String = text.chars().take(keep / 2).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(keep / 2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}
fn reward_line(ui: &mut egui::Ui, reward: &Value) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Block {}", reward["height"])).monospace());
        gnr_amount(ui, reward["amount_atomic"].as_str(), 13.0, None);
        ui.label(
            RichText::new(reward["status_label"].as_str().unwrap_or_default())
                .small()
                .color(GRAY),
        );
    });
}
fn reward_row(ui: &mut egui::Ui, reward: &Value) {
    let status = reward["status"].as_str().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Mined block reward").strong());
                ui.label(
                    RichText::new(reward["status_label"].as_str().unwrap_or_default())
                        .small()
                        .color(GRAY),
                );
            });
            ui.label(
                RichText::new(format!(
                    "Block {} · {} · {} confirmation(s)",
                    reward["height"],
                    views_wallet::format_utc(reward["timestamp"].as_u64().unwrap_or(0)),
                    reward["confirmations"]
                ))
                .small()
                .color(GRAY),
            );
            match status {
                "Replaced" => {
                    ui.label(
                        RichText::new("A heavier chain put a different block at this height. This reward is kept as history only and is not part of any balance.")
                            .small()
                            .color(GRAY),
                    );
                }
                "Unconfirmed" => {
                    ui.label(
                        RichText::new("The chain was reorganized below this block; waiting until it reaches this height again. Not counted meanwhile.")
                            .small()
                            .color(GRAY),
                    );
                }
                _ => {
                    let progress = reward["progress_permille"].as_u64().unwrap_or(0) as f32 / 1000.0;
                    ui.add(
                        egui::ProgressBar::new(progress)
                            .desired_width(240.0)
                            .text(if status == "Mature" {
                                "Spendable".to_owned()
                            } else {
                                format!("Spendable at block {}", reward["unlock_height"])
                            }),
                    );
                }
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let color = (status == "Replaced").then_some(GRAY);
            gnr_amount(ui, reward["amount_atomic"].as_str(), 18.0, color);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reward_filters_and_totals_never_mix_replaced_rewards() {
        let rewards = vec![
            json!({"status": "JustMined", "amount_atomic": "1"}),
            json!({"status": "Maturing", "amount_atomic": "2"}),
            json!({"status": "Mature", "amount_atomic": "4"}),
            json!({"status": "Replaced", "amount_atomic": "8"}),
            json!({"status": "Unconfirmed", "amount_atomic": "16"}),
        ];
        assert_eq!(
            reward_total(&rewards, &["JustMined", "Maturing", "Mature"]),
            7
        );
        assert_eq!(reward_total(&rewards, &["Replaced"]), 8);
        let count = |filter: RewardFilter| {
            rewards
                .iter()
                .filter(|r| filter.matches(r["status"].as_str().unwrap()))
                .count()
        };
        assert_eq!(count(RewardFilter::All), 5);
        assert_eq!(count(RewardFilter::JustMined), 1);
        assert_eq!(count(RewardFilter::Maturing), 2);
        assert_eq!(count(RewardFilter::Mature), 1);
        assert_eq!(count(RewardFilter::Replaced), 1);
    }

    #[test]
    fn unknown_and_stale_balances_are_labelled_honestly() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.sync_detail = Some(json!({"stale": true, "verified_by": "this computer"}));
        app.wallet_height = Some(42);
        assert!(app.balance_is_stale());
        app.sync_detail = Some(json!({"stale": false}));
        assert!(!app.balance_is_stale());
        assert_eq!(short_text("abcdefghij", 4), "ab…ij");
    }
}
