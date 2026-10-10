//! Explicit controls for the node owned by this application.
use super::*;
use std::io::{Seek, SeekFrom};

const MAX_LOG_BYTES: u64 = 65_536;

pub(super) const NO_LOG: &str = "No node log in this data folder yet. The wallet writes genorynd.log for nodes it starts; a node started separately logs wherever it was started.";

/// `22:29:14  INFO  accepted block  height=18` from one JSON tracing line; other
/// lines are returned unchanged.
pub(super) fn readable_log_line(line: &str) -> String {
    let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    let time = entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|stamp| stamp.get(11..19))
        .unwrap_or("--:--:--");
    let level = entry.get("level").and_then(Value::as_str).unwrap_or("");
    let mut text = format!("{time}  {level:<5}");
    if let Some(Value::Object(fields)) = entry.get("fields") {
        if let Some(message) = fields.get("message").and_then(Value::as_str) {
            text.push_str("  ");
            text.push_str(message);
        }
        for (key, value) in fields.iter().filter(|(key, _)| *key != "message") {
            let value = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            text.push_str(&format!("  {key}={value}"));
        }
    }
    text
}

fn read_log_tail(directory: &str) -> Result<String> {
    let path = PathBuf::from(directory).join("genorynd.log");
    ensure!(path.exists(), "{NO_LOG}");
    ensure!(
        std::fs::symlink_metadata(&path)?.file_type().is_file(),
        "Node log must be a regular local file."
    );
    let mut file = std::fs::File::open(path).context("Cannot open the selected node's log.")?;
    let length = file.metadata()?.len();
    let offset = length.saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
    if offset > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        } else {
            bytes.clear();
        }
    }
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(readable_log_line)
        .collect::<Vec<_>>()
        .join("\n"))
}

impl WalletApp {
    /// Restarts the owned node with the saved network settings, pausing and
    /// resuming the miner around it.
    pub(super) fn restart_owned_node(&mut self) {
        if self.node.is_none() || self.network_transition.is_some() {
            return;
        }
        let mode = network_privacy::Mode::parse(&self.preferences.network_mode)
            .unwrap_or(network_privacy::Mode::Direct);
        self.apply_network_mode(mode);
    }

    pub(super) fn load_node_log(&mut self) {
        if self.log_busy {
            return;
        }
        self.log_busy = true;
        let directory = self.settings.data_dir.clone();
        let generation = self.log_generation;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = read_log_tail(&directory).map_err(|error| error.to_string());
            let _ = tx.send(Event::NodeLog {
                directory,
                generation,
                result,
            });
        });
    }
    pub(super) fn close_node_log(&mut self) {
        self.log_generation = self.log_generation.wrapping_add(1);
        self.node_log = None;
        self.node_log_error = None;
    }
    /// The recent node log in its own window, newest lines at the bottom.
    pub(super) fn node_log_window(&mut self, ctx: &egui::Context) {
        if self.node_log.is_none() && self.node_log_error.is_none() {
            return;
        }
        let path = PathBuf::from(&self.settings.data_dir).join("genorynd.log");
        let mut open = true;
        let mut refresh = false;
        egui::Window::new("Node log")
            .open(&mut open)
            .collapsible(false)
            .default_size([820.0, 460.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("{} · last 64 KiB · read locally", path.display()))
                        .small()
                        .color(GRAY),
                );
                ui.add_space(6.0);
                if let Some(error) = &self.node_log_error {
                    ui.label(error.as_str());
                } else if let Some(text) = &mut self.node_log {
                    egui::ScrollArea::vertical()
                        .id_salt("node_log_tail")
                        .max_height(360.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(text)
                                    .font(egui::TextStyle::Monospace)
                                    .interactive(false)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.log_busy, egui::Button::new("Refresh"))
                        .clicked()
                    {
                        refresh = true;
                    }
                    if path.is_file() && ui.button("Open log file").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(file_url(&path)));
                    }
                    if ui.button("Open folder").clicked() {
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(file_url(&PathBuf::from(
                                &self.settings.data_dir,
                            ))));
                    }
                });
            });
        if !open {
            self.close_node_log();
        } else if refresh {
            self.load_node_log();
        }
    }
}

/// A `file:` URL the operating system opens with its default application.
fn file_url(path: &std::path::Path) -> String {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = absolute.display().to_string();
    let text = text
        .strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_view_is_bounded_and_drops_partial_leading_line() {
        let directory = std::env::temp_dir().join(format!(
            "genoryn-log-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("genorynd.log");
        let mut bytes = vec![b'x'; 70_000];
        bytes.extend_from_slice(b"\nlast complete line\n");
        std::fs::write(&path, bytes).unwrap();
        let result = read_log_tail(directory.to_str().unwrap()).unwrap();
        assert_eq!(result, "last complete line");
        assert!(result.len() <= MAX_LOG_BYTES as usize);
        std::fs::remove_file(path).unwrap();
        let missing = read_log_tail(directory.to_str().unwrap()).unwrap_err();
        assert_eq!(missing.to_string(), NO_LOG, "a missing log is explained");
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn node_log_lines_are_readable() {
        let line = r#"{"timestamp":"2026-10-09T22:29:14.660990Z","level":"INFO","fields":{"message":"accepted block","height":18,"reorg_depth":0},"target":"genoryn::node"}"#;
        assert_eq!(
            readable_log_line(line),
            "22:29:14  INFO   accepted block  height=18  reorg_depth=0"
        );
        assert_eq!(readable_log_line("plain text"), "plain text");
        assert!(file_url(std::path::Path::new("/tmp/x.log")).starts_with("file:///"));
    }

    #[test]
    fn external_node_cannot_be_restarted() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.restart_owned_node();
        assert!(app.network_transition.is_none());
        assert!(!app.process_busy);
    }
    #[test]
    fn node_auto_start_is_decided_once_and_only_when_none_answers() {
        let ctx = egui::Context::default();
        let unstarted = |configure: fn(&mut Settings), reachable: bool| {
            let mut settings = Settings::default();
            configure(&mut settings);
            let mut app = WalletApp::with_context(&ctx, settings);
            app.auto_start_node(reachable);
            assert!(app.auto_node_checked);
            !app.process_busy
        };
        assert!(unstarted(|_| {}, true), "a running node is used as is");
        assert!(unstarted(|s| s.auto_node = false, false));
        assert!(unstarted(|s| s.smoke_test_frames = Some(5), false));

        let mut settings = Settings::default();
        settings.data_dir = std::env::temp_dir()
            .join(format!("genoryn-auto-node-{}", std::process::id()))
            .display()
            .to_string();
        settings.bin_dir = settings.data_dir.clone();
        let mut app = WalletApp::with_context(&ctx, settings);
        app.auto_node_checked = true;
        app.auto_start_node(false);
        assert!(!app.process_busy, "a later outage never restarts silently");
        app.auto_node_checked = false;
        app.auto_start_node(false);
        assert!(app.process_busy, "first unreachable result starts the node");
        if let Some(worker) = app.process_worker.take() {
            let _ = worker.join();
        }
        let _ = std::fs::remove_dir_all(&app.settings.data_dir);
    }
    #[test]
    fn wallet_rescans_only_when_behind_unlocked_and_not_too_often() {
        let ctx = egui::Context::default();
        let settings = Settings {
            bin_dir: std::env::temp_dir()
                .join("genoryn-no-binaries-here")
                .display()
                .to_string(),
            ..Settings::default()
        };
        let mut app = WalletApp::with_context(&ctx, settings);
        app.node_info = Some(
            serde_json::from_value(json!({
                "endpoint": "http://127.0.0.1:29411",
                "latency_ms": 1,
                "info": {
                    "network": "devnet", "chain_id": "genoryn-devnet-v1",
                    "genesis_hash": "00", "version": 1, "height": 10,
                    "best_hash": "00", "difficulty": "1", "peer_count": 0,
                    "mempool_count": 0, "public_rpc": false
                }
            }))
            .unwrap(),
        );
        let long_ago = Instant::now() - AUTO_SCAN_INTERVAL - Duration::from_secs(1);
        let attempted = |app: &mut WalletApp| {
            app.last_auto_scan = long_ago;
            app.auto_scan();
            app.last_auto_scan != long_ago
        };
        app.wallet_height = Some(5);
        assert!(!attempted(&mut app), "a locked wallet never scans");
        app.unlocked = true;
        app.wallet_height = Some(10);
        assert!(!attempted(&mut app), "already at the node's height");
        app.wallet_height = Some(5);
        app.preview = Some(PaymentPreview {
            to: String::new(),
            amount: "1".into(),
            fee: "1".into(),
            memo: String::new(),
        });
        assert!(
            !attempted(&mut app),
            "a payment under review is not interrupted"
        );
        app.preview = None;
        assert!(attempted(&mut app), "behind the node after the interval");
        assert!(app.status.contains("genoryn-wallet-cli"));
        app.auto_scan();
        assert!(
            app.last_auto_scan.elapsed() < AUTO_SCAN_INTERVAL,
            "the interval throttles repeated scans"
        );
        assert_eq!(app.auto_scan_interval(), AUTO_SCAN_INTERVAL);
        app.last_scan_duration = Duration::from_secs(10);
        assert_eq!(
            app.auto_scan_interval(),
            Duration::from_secs(30),
            "slow scans back off so scanning stays a minority of the time"
        );
    }
    #[test]
    fn background_scans_never_hold_or_release_the_action_lane() {
        assert!(controller::is_scan_label("sync"));
        assert!(controller::is_scan_label("sync@C:/wallets/savings.gnr"));
        assert!(!controller::is_scan_label("send"));
        assert!(!controller::is_scan_label("unlock@x.gnr"));
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.busy = Some("send".into());
        app.scan_busy = Some("sync".into());
        app.tx
            .send(Event::Wallet {
                generation: app.generation,
                op: "sync".into(),
                result: Err("scan failed".into()),
            })
            .unwrap();
        app.poll();
        assert!(app.scan_busy.is_none(), "the scan lane is free again");
        assert_eq!(
            app.busy.as_deref(),
            Some("send"),
            "the payment keeps running"
        );
        app.scan_busy = Some("sync".into());
        app.tx
            .send(Event::Wallet {
                generation: app.generation,
                op: "send".into(),
                result: Err("rejected".into()),
            })
            .unwrap();
        app.poll();
        assert!(app.busy.is_none());
        assert_eq!(
            app.scan_busy.as_deref(),
            Some("sync"),
            "the scan keeps running"
        );
    }
    #[test]
    fn dismissed_or_changed_directory_log_cannot_be_restored_by_late_result() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        let directory = app.settings.data_dir.clone();
        let generation = app.log_generation;
        app.node_log = Some("old visible log".into());
        app.log_busy = true;
        app.close_node_log();
        app.tx
            .send(Event::NodeLog {
                directory: directory.clone(),
                generation,
                result: Ok("late log".into()),
            })
            .unwrap();
        app.poll();
        assert!(app.node_log.is_none());
        assert!(!app.log_busy);
        app.settings.data_dir = "another-node".into();
        app.tx
            .send(Event::NodeLog {
                directory,
                generation: app.log_generation,
                result: Ok("wrong directory".into()),
            })
            .unwrap();
        app.poll();
        assert!(app.node_log.is_none());
    }
}
