//! Short, one-time notices about a wallet's payments: received, on the way,
//! confirmed, and new block rewards. Each event is announced once; the first scan
//! after unlocking only records what already exists, so old history never floods
//! the screen. Notices disappear by themselves.
use super::*;
use std::collections::{HashMap, HashSet};

/// How long a notice stays on screen.
pub(super) const TOAST_SECONDS: u64 = 9;
const MAX_TOASTS: usize = 4;

pub(super) struct Toast {
    pub text: String,
    pub created: Instant,
}

#[derive(Default)]
pub(super) struct Notifier {
    /// Events already seen, per wallet file.
    seen: HashMap<String, HashSet<String>>,
    pub toasts: Vec<Toast>,
}

fn amount(value: &Value) -> u128 {
    value["amount_atomic"]
        .as_str()
        .and_then(|amount| amount.parse().ok())
        .unwrap_or(0)
}
fn gnr(atomic: u128) -> String {
    format!("{} GNR", format_atomic(&atomic.to_string()))
}
fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

impl Notifier {
    /// Compares a scan result of the wallet at `path` with what was seen before
    /// and queues one notice summarising anything new.
    pub fn observe(&mut self, path: &str, result: &Value) {
        let first = !self.seen.contains_key(path);
        let seen = self.seen.entry(path.to_owned()).or_default();
        let (mut received, mut confirmed, mut rewards, mut on_way) =
            ((0, 0u128), (0, 0u128), (0, 0u128), (0, 0u128));
        for entry in result["history"].as_array().into_iter().flatten() {
            let direction = entry["direction"].as_str().unwrap_or_default();
            let key = format!(
                "h:{}:{direction}",
                entry["txid"].as_str().unwrap_or_default()
            );
            if !seen.insert(key) || first {
                continue;
            }
            match direction {
                "received" => {
                    received.0 += 1;
                    received.1 += amount(entry);
                }
                "sent" => {
                    confirmed.0 += 1;
                    confirmed.1 += amount(entry);
                }
                _ => {}
            }
        }
        for reward in result["mined"].as_array().into_iter().flatten() {
            if reward["status"] == "Replaced" {
                continue;
            }
            let key = format!(
                "m:{}:{}",
                reward["txid"].as_str().unwrap_or_default(),
                reward["action"]
            );
            if seen.insert(key) && !first {
                rewards.0 += 1;
                rewards.1 += amount(reward);
            }
        }
        for entry in result["outbox"].as_array().into_iter().flatten() {
            let status = entry["status"].as_str().unwrap_or_default();
            if !matches!(status, "Pending" | "Submitted") {
                continue;
            }
            // Announced once per payment, not again when it moves to Submitted.
            let key = format!("o:{}", entry["txid"].as_str().unwrap_or_default());
            if seen.insert(key) && !first {
                on_way.0 += 1;
                on_way.1 += amount(entry);
            }
        }
        let mut parts = Vec::new();
        if received.0 > 0 {
            parts.push(format!(
                "received {} ({})",
                plural(received.0, "payment", "payments"),
                gnr(received.1)
            ));
        }
        if on_way.0 > 0 {
            parts.push(format!(
                "{} on the way ({})",
                plural(on_way.0, "payment", "payments"),
                gnr(on_way.1)
            ));
        }
        if confirmed.0 > 0 {
            parts.push(format!(
                "{} confirmed ({})",
                plural(confirmed.0, "payment", "payments"),
                gnr(confirmed.1)
            ));
        }
        if rewards.0 > 0 {
            parts.push(format!(
                "{} ({})",
                plural(rewards.0, "new block reward", "new block rewards"),
                gnr(rewards.1)
            ));
        }
        if parts.is_empty() {
            return;
        }
        let text = format!("{}: {}", sessions::wallet_name(path), parts.join(" · "));
        self.toasts.push(Toast {
            text,
            created: Instant::now(),
        });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }
    /// Locking forgets everything, including which events were announced.
    pub fn clear(&mut self) {
        self.seen.clear();
        self.toasts.clear();
    }
    pub fn expire(&mut self) {
        self.toasts
            .retain(|toast| toast.created.elapsed() < Duration::from_secs(TOAST_SECONDS));
    }
}

impl WalletApp {
    /// Notices in the top-right corner, below the top bar.
    pub(super) fn toasts_overlay(&mut self, ctx: &egui::Context) {
        self.notifier.expire();
        if self.notifier.toasts.is_empty() {
            return;
        }
        let mut dismiss = None;
        egui::Area::new(egui::Id::new("genoryn-toasts"))
            .anchor(egui::Align2::RIGHT_TOP, [-24.0, 76.0])
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for (index, toast) in self.notifier.toasts.iter().enumerate() {
                    let visuals = ui.visuals().clone();
                    egui::Frame::new()
                        .fill(visuals.window_fill)
                        .stroke(visuals.window_stroke)
                        .corner_radius(egui::CornerRadius::same(10))
                        .shadow(visuals.popup_shadow)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add(egui::Label::new(RichText::new(&toast.text)).wrap());
                                if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                                    dismiss = Some(index);
                                }
                            });
                        });
                    ui.add_space(8.0);
                }
            });
        if let Some(index) = dismiss {
            self.notifier.toasts.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(history: Value, mined: Value, outbox: Value) -> Value {
        json!({"history": history, "mined": mined, "outbox": outbox})
    }

    #[test]
    fn the_first_scan_is_silent_and_each_event_is_announced_once() {
        let mut notifier = Notifier::default();
        let old = scan(
            json!([{"txid": "aa", "direction": "received", "amount_atomic": "5000000000000"}]),
            json!([]),
            json!([]),
        );
        notifier.observe("C:/w/main.gnr", &old);
        assert!(
            notifier.toasts.is_empty(),
            "existing history is not announced"
        );
        let new = scan(
            json!([
                {"txid": "aa", "direction": "received", "amount_atomic": "5000000000000"},
                {"txid": "bb", "direction": "received", "amount_atomic": "2500000000000"},
                {"txid": "cc", "direction": "sent", "amount_atomic": "1000000000000"}
            ]),
            json!([{"txid": "dd", "action": 0, "status": "JustMined", "amount_atomic": "1000000000000000"}]),
            json!([{"txid": "ee", "status": "Pending", "amount_atomic": "3000000000000"}]),
        );
        notifier.observe("C:/w/main.gnr", &new);
        assert_eq!(notifier.toasts.len(), 1, "one summary per scan");
        let text = &notifier.toasts[0].text;
        assert!(text.starts_with("main: "), "{text}");
        assert!(text.contains("received 1 payment (2.5 GNR)"), "{text}");
        assert!(text.contains("1 payment on the way (3 GNR)"), "{text}");
        assert!(text.contains("1 payment confirmed (1 GNR)"), "{text}");
        assert!(text.contains("1 new block reward (1000 GNR)"), "{text}");
        let mut submitted = new.clone();
        submitted["outbox"][0]["status"] = json!("Submitted");
        notifier.observe("C:/w/main.gnr", &submitted);
        assert_eq!(notifier.toasts.len(), 1, "nothing new: no second notice");
    }

    #[test]
    fn wallets_are_tracked_separately_and_replaced_rewards_are_never_announced() {
        let mut notifier = Notifier::default();
        notifier.observe("a.gnr", &scan(json!([]), json!([]), json!([])));
        notifier.observe(
            "a.gnr",
            &scan(
                json!([]),
                json!([{"txid": "r1", "action": 0, "status": "Replaced", "amount_atomic": "1"}]),
                json!([]),
            ),
        );
        assert!(notifier.toasts.is_empty());
        notifier.observe(
            "b.gnr",
            &scan(
                json!([{"txid": "x", "direction": "received", "amount_atomic": "1"}]),
                json!([]),
                json!([]),
            ),
        );
        assert!(
            notifier.toasts.is_empty(),
            "first scan of another wallet is silent"
        );
        notifier.clear();
        assert!(notifier.toasts.is_empty());
    }
}
