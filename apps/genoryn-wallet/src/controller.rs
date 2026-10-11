//! Native wallet controller; uses the shared backend.
use super::*;

/// Results of the background scan lane: the active wallet's or another open wallet's.
pub(super) fn is_scan_label(label: &str) -> bool {
    label == "sync" || label.starts_with("sync@") || is_batch_label(label)
}
/// Batches of many wallets, which outlive switching the active wallet.
pub(super) fn is_batch_label(label: &str) -> bool {
    label == sessions::BATCH_UNLOCK || label == sessions::BATCH_SCAN
}
/// The parts of a scan result shown besides balance and history.
pub(super) fn sync_detail(result: &Value) -> Value {
    let mut detail = serde_json::Map::new();
    for key in [
        "mined",
        "outbox",
        "mode",
        "verified_by",
        "stale",
        "offline_reason",
        "reserved_atomic",
        "pending_atomic",
        "light_sources",
        "sync_stats",
        "birthday",
        "spendable_notes",
        "max_single_payment_atomic",
    ] {
        if let Some(value) = result.get(key) {
            detail.insert(key.into(), value.clone());
        }
    }
    Value::Object(detail)
}

impl WalletApp {
    pub(super) fn lock(&mut self) {
        self.cancellation.store(true, Ordering::Release);
        self.scan_cancellation.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.password.zeroize();
        self.confirm_password.zeroize();
        self.seed_input.zeroize();
        self.seed_backup.zeroize();
        self.unlocked = false;
        self.wallet_mode = initial_wallet_mode(&self.settings.wallet_path);
        self.seed_ack = false;
        self.address.zeroize();
        self.balance = None;
        self.spendable = None;
        self.wallet_height = None;
        for value in &mut self.history {
            protocol::wipe_json(value);
        }
        self.history.clear();
        if let Some(detail) = &mut self.sync_detail {
            protocol::wipe_json(detail);
        }
        self.sync_detail = None;
        self.notifier.clear();
        self.preview = None;
        self.proofs = payment_proof::ProofUi::default();
        self.to.zeroize();
        self.amount.zeroize();
        self.memo.zeroize();
        self.request_amount.zeroize();
        self.last_sync = None;
        self.last_submission = None;
        self.payment_request = None;
        self.forget_name_identity();
        self.sessions.clear();
        self.pending_unlocks.clear();
        self.pending_password = None;
        self.unlock_inflight.clear();
        self.unlock_failures.clear();
        self.unlock_progress = None;
        self.lock_epoch = self.lock_epoch.wrapping_add(1);
        self.adding_wallet = false;
        self.prepare_wallet_form(self.wallet_mode);
        self.status =
            "Wallet locked. A local proof save already started may have completed. Owned node and mining processes keep running until stopped."
                .into();
    }
    pub(super) fn request(&mut self, op: &str, extra: Value) {
        // Verifying needs no wallet; an unlocked one also learns whether it was paid.
        let credentials = (op != "verify_proof" || self.unlocked)
            .then(|| (self.settings.wallet_path.clone(), self.password.clone()));
        self.dispatch(op, op.to_owned(), credentials, extra);
    }
    /// Runs one CLI operation for the given wallet; `label` identifies its result.
    pub(super) fn dispatch(
        &mut self,
        op: &str,
        label: String,
        credentials: Option<(String, Zeroizing<String>)>,
        extra: Value,
    ) {
        let extra = SecretJson(extra);
        // Scans run in their own lane so they never block the user's actions.
        let scan = op == "sync" || op == "sync_batch";
        if (scan && self.scan_busy.is_some()) || (!scan && self.busy.is_some()) {
            return;
        }
        let light = match network_privacy::light_fields(&self.preferences, &self.settings, op) {
            Ok(light) => light,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        if light.is_none()
            && let Err(error) = network_privacy::guard_rpc(&self.settings.rpc_url)
        {
            self.status = error.to_string();
            return;
        }
        let mut request = json!({"op":op,"network":NETWORK.name(),"rpc_url":self.settings.rpc_url,"confirmation_target":self.settings.confirmation_target});
        if let Some((path, password)) = &credentials {
            request["wallet_path"] = json!(path);
            request["password"] = json!(password.as_str());
        }
        let loopback = genoryn_connect::validate_endpoint(&self.settings.rpc_url, NETWORK.name())
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .and_then(|h| {
                h.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|ip| ip.is_loopback());
        if loopback && credentials.is_some() {
            request["token_file"] = json!(
                PathBuf::from(&self.settings.data_dir)
                    .join("admin.token")
                    .display()
                    .to_string()
            );
        }
        let mut request = SecretJson(request);
        if let Some(extra) = extra.0.as_object() {
            for (key, value) in extra {
                request.0[key] = value.clone();
            }
        }
        if let Some(Value::Object(fields)) = light {
            for (key, value) in fields {
                request.0[key] = value;
            }
        }
        let prepared = protocol::sibling_binary("genoryn-wallet-cli", &self.settings.bin_dir)
            .and_then(|binary| Ok((binary, protocol::encode_request(request.0.take())?)));
        let (binary, bytes) = match prepared {
            Ok(result) => result,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        if scan {
            self.scan_busy = Some(label.clone());
            self.scan_started = Some(Instant::now());
            self.scan_cancellation = cancelled.clone();
            self.scan_epoch = self.lock_epoch;
        } else {
            self.busy = Some(op.into());
            self.cancellation = cancelled.clone();
        }
        let tx = self.tx.clone();
        let generation = self.generation;
        let op = label;
        let worker = thread::spawn(move || {
            let result =
                protocol::cli_request(&binary, bytes, cancelled).map_err(|e| e.to_string());
            let _ = tx.send(Event::Wallet {
                generation,
                op,
                result,
            });
        });
        if scan {
            self.scan_worker = Some(worker);
        } else {
            self.wallet_worker = Some(worker);
        }
    }
    pub(super) fn refresh_network(&mut self) {
        if self.health_busy {
            return;
        }
        self.health_busy = true;
        self.last_health = Instant::now();
        let tx = self.tx.clone();
        let endpoint = self.settings.rpc_url.clone();
        let generation = self.network_generation;
        thread::spawn(move || {
            let result = (|| -> Result<(HealthyNode, Value)> {
                network_privacy::guard_rpc(&endpoint)?;
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(async {
                    let network = NETWORK;
                    let mut client = Connect::new(ConnectConfig::for_network(
                        NETWORK.name(),
                        network.chain_id(),
                        hex::encode(network.genesis()),
                        vec![endpoint.clone()],
                    ))?;
                    let selected = client.select().await?;
                    let supply: Value = client.get_json("/v1/supply").await?;
                    Ok((selected, supply))
                })
            })()
            .map_err(|e| e.to_string());
            let privacy = (result.is_ok() && network_privacy::is_loopback_url(&endpoint))
                .then(|| network_privacy::fetch_status(&endpoint).ok())
                .flatten();
            let _ = tx.send(Event::Health {
                generation,
                endpoint,
                result: Box::new(result),
                privacy,
            });
        });
    }
    pub(super) fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::NodeLog {
                    directory,
                    generation,
                    result,
                } => {
                    self.log_busy = false;
                    if directory == self.settings.data_dir && generation == self.log_generation {
                        match result {
                            Ok(text) => {
                                self.node_log = Some(text);
                                self.node_log_error = None;
                            }
                            Err(error) => {
                                self.node_log = None;
                                self.node_log_error = Some(error);
                            }
                        }
                    }
                }
                Event::Wallet {
                    generation,
                    op,
                    result,
                } => {
                    let scan = is_scan_label(&op);
                    let mut automatic = false;
                    let batch = is_batch_label(&op);
                    if scan {
                        self.scan_busy = None;
                        automatic = std::mem::take(&mut self.auto_scan_running);
                        if let Some(started) = self.scan_started.take() {
                            // Only the active wallet's scans pace its automatic scans.
                            if op == "sync" {
                                self.last_scan_duration = started.elapsed();
                            } else if batch {
                                self.batch_scan_duration = started.elapsed();
                            }
                        }
                    } else {
                        self.busy = None;
                    }
                    // A batch belongs to the whole unlocked session, not to the
                    // active wallet: only locking discards it.
                    let stale = if batch {
                        self.scan_epoch != self.lock_epoch
                    } else {
                        generation != self.generation
                    };
                    if stale {
                        self.requeue_stale_unlock(&op);
                        continue;
                    }
                    match result {
                        Err(error) => {
                            if matches!(op.as_str(), "send" | "register_name" | "combine_notes") {
                                self.action_error = Some((op.clone(), error.clone()));
                            }
                            if op == "verify_proof" {
                                self.proofs.verify_error = Some(error.clone());
                            }
                            if !self.batch_failed(&op, &error) {
                                self.status = error;
                            }
                        }
                        Ok(response) if self.apply_background_sync(&op, &response.0) => {}
                        Ok(response) if self.apply_batch(&op, &response.0) => {}
                        Ok(response) => {
                            let result = &response.0;
                            match op.as_str() {
                                "create" => {
                                    self.address =
                                        result["address"].as_str().unwrap_or_default().into();
                                    *self.seed_backup =
                                        result["seed_phrase"].as_str().unwrap_or_default().into();
                                    if self.seed_backup.is_empty() {
                                        self.status="CLI did not return a recovery phrase. Wallet remains locked.".into();
                                    } else {
                                        self.status =
                                            "Write down your recovery phrase before continuing."
                                                .into();
                                    }
                                }
                                "restore" | "open" => {
                                    self.address =
                                        result["address"].as_str().unwrap_or_default().into();
                                    self.unlocked = !self.address.is_empty();
                                    self.seed_input.zeroize();
                                    self.confirm_password.zeroize();
                                    self.status=if self.unlocked {"Wallet unlocked. Scan the chain to reconstruct your balance."}else{"CLI returned no receiving address; wallet remains locked."}.into();
                                    if self.unlocked && !self.reject_duplicate_wallet() {
                                        self.after_unlock(op == "open");
                                        self.request("sync", json!({}));
                                    }
                                }
                                "sync" => {
                                    self.set_name_identity(result);
                                    self.balance =
                                        result["balance_atomic"].as_str().map(str::to_owned);
                                    self.spendable =
                                        result["spendable_atomic"].as_str().map(str::to_owned);
                                    self.wallet_height = result["height"].as_u64();
                                    self.history =
                                        result["history"].as_array().cloned().unwrap_or_default();
                                    self.sync_detail = Some(sync_detail(result));
                                    self.notifier.observe(&self.settings.wallet_path, result);
                                    self.last_sync = Some(Instant::now());
                                    if let Some(address) = result["address"].as_str() {
                                        self.address = address.into();
                                    }
                                    self.status = if automatic {
                                        format!(
                                            "Wallet scanned automatically to block {}.",
                                            self.wallet_height.unwrap_or_default()
                                        )
                                    } else {
                                        "Local private-note scan finished. Only your wallet's own history is shown.".into()
                                    };
                                }
                                "send" => {
                                    self.action_error = None;
                                    self.last_submission =
                                        result["txid"].as_str().map(str::to_owned);
                                    self.status = format!(
                                        "Payment submitted: {}. Confirmation requires a mined block.",
                                        result["txid"]
                                            .as_str()
                                            .unwrap_or("transaction ID unavailable")
                                    );
                                    self.preview = None;
                                    self.memo.zeroize();
                                    self.amount.clear();
                                    self.request("sync", json!({}));
                                }
                                "register_name" => {
                                    self.action_error = None;
                                    let name =
                                        result["name"].as_str().unwrap_or_default().to_owned();
                                    self.status = format!(
                                        "@{name} submitted: {} GNR burned. It becomes yours when the next block confirms it.",
                                        format_atomic(
                                            result["burned_atomic"].as_str().unwrap_or("0")
                                        )
                                    );
                                    self.names.cache.remove(&name);
                                    self.names.input.clear();
                                    self.names.mine_checked = None;
                                }
                                "register_username" => {
                                    self.status = format!(
                                        "Username {} registered in the local signed registry.",
                                        result["username"].as_str().unwrap_or(&self.username)
                                    )
                                }
                                "resolve_username" => {
                                    if result["verified"].as_bool() == Some(true) {
                                        self.status = format!(
                                            "Signature checked. Confirm owner fingerprint independently: {}. Receiver: {}",
                                            result["owner_fingerprint"]
                                                .as_str()
                                                .unwrap_or("unavailable"),
                                            result["address"].as_str().unwrap_or("unavailable")
                                        );
                                    } else {
                                        self.status =
                                            "Resolver did not authenticate the receiving identity."
                                                .into();
                                    }
                                }
                                "combine_notes" => {
                                    self.action_error = None;
                                    self.status = format!(
                                        "Combining {} notes in {} transfer(s) to yourself. After the next block, larger payments work; send again then.",
                                        result["notes_combined"], result["transactions"]
                                    );
                                    self.request("sync", json!({}));
                                }
                                "export_proof" => self.accept_exported_proof(result),
                                "verify_proof" => self.accept_verified_proof(result),
                                _ => self.status = "Wallet operation finished.".into(),
                            }
                        }
                    }
                }
                Event::ProofFile { generation, result } => {
                    self.busy = None;
                    if generation != self.generation {
                        continue;
                    }
                    match result {
                        Ok(payment_proof::FileResult::Loaded(text)) => {
                            self.proofs.input = text;
                            self.proofs.verified = None;
                            self.proofs.verify_error = None;
                            self.status =
                                "Proof file loaded. Press Verify payment to check it against the chain."
                                    .into();
                        }
                        Ok(payment_proof::FileResult::Saved(path)) => {
                            self.status = format!("Proof saved to {}.", path.display());
                            self.proofs.saved = Some(path);
                        }
                        Err(error) => {
                            if self.page == VERIFY_PAGE {
                                self.proofs.verify_error = Some(error.clone());
                            }
                            self.status = error;
                        }
                    }
                }
                Event::Reconnect(result) => {
                    self.reconnect_busy = false;
                    self.status = match result {
                        Ok(()) => "The node is re-checking Tor and retrying peers now.".into(),
                        Err(error) => error,
                    };
                    self.last_health = Instant::now() - Duration::from_secs(10);
                }
                Event::Health {
                    generation,
                    endpoint,
                    result,
                    privacy,
                } => {
                    self.health_busy = false;
                    if generation != self.network_generation
                        || endpoint.trim_end_matches('/')
                            != self.settings.rpc_url.trim_end_matches('/')
                    {
                        self.last_health = Instant::now() - Duration::from_secs(10);
                        continue;
                    }
                    self.network_privacy = privacy;
                    let reachable = result.is_ok();
                    match *result {
                        Ok((info, supply)) => {
                            self.node_info = Some(info);
                            self.supply = Some(supply);
                            self.network_error = None;
                        }
                        Err(error) => {
                            self.node_info = None;
                            self.supply = None;
                            self.network_error = Some(error);
                        }
                    }
                    self.auto_start_node(reachable);
                }
                Event::Started { kind, result } => {
                    self.process_busy = false;
                    match result {
                        Ok(child) => {
                            if kind == "node" {
                                self.node = Some(child);
                            } else {
                                self.miner = Some(child);
                                self.mining_stats = None;
                            }
                            self.status =
                                format!("{kind} process started. Waiting for live status.");
                            self.last_health = Instant::now() - Duration::from_secs(10);
                        }
                        Err(error) => {
                            if kind == "miner" {
                                self.miner_note = Some(error.clone());
                            }
                            self.status = error;
                        }
                    }
                }
                Event::Stopped { kind, result } => {
                    self.process_busy = false;
                    if let Err(error) = &result
                        && self.network_transition.is_some()
                    {
                        self.network_transition = None;
                        self.network_outcome = Some(network_transition::Outcome::Failed(format!(
                            "could not stop the {kind}: {error}"
                        )));
                    }
                    self.status = match result {
                        Ok(()) => format!("Owned {kind} process stopped."),
                        Err(e) => e,
                    };
                    if kind == "miner" {
                        self.mining_stats = None;
                    }
                    self.last_health = Instant::now() - Duration::from_secs(10);
                }
                Event::Mining(value) => self.mining_stats = Some(value),
                event @ (Event::NameStatus { .. } | Event::MyNames { .. }) => {
                    self.names_event(event)
                }
                Event::Update(result) => {
                    self.update_status = match result {
                        Ok(Some(release)) => {
                            self.update_dismissed = false;
                            updates::UpdateStatus::Available(release)
                        }
                        Ok(None) => updates::UpdateStatus::UpToDate,
                        Err(error) => updates::UpdateStatus::Failed(error),
                    };
                }
            }
        }
        let mut miner_exit = None;
        for (kind, slot) in [("node", &mut self.node), ("miner", &mut self.miner)] {
            if let Some(child) = slot {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        self.status = format!(
                            "Owned {kind} process exited ({status}). Inspect its log in the data directory."
                        );
                        *slot = None;
                        if kind == "miner" {
                            miner_exit = Some(status);
                        }
                    }
                    Err(e) => self.status = format!("Cannot inspect {kind} process: {e}"),
                    _ => {}
                }
            }
        }
        if let Some(status) = miner_exit {
            self.miner_exited(status);
        }
    }
    /// A user-started run: automatic fixes begin again from the chosen settings.
    pub(super) fn start_mining(&mut self) {
        self.miner_attempt = miner_recovery::Attempt::fresh(self.settings.threads);
        self.miner_destination = Some((
            self.address.clone(),
            sessions::wallet_name(&self.settings.wallet_path),
        ));
        self.miner_note = None;
        self.miner_retry_at = None;
        self.start_process("miner");
    }
    pub(super) fn stop_mining(&mut self) {
        self.miner_retry_at = None;
        self.stop_process("miner");
    }
    fn miner_exited(&mut self, status: std::process::ExitStatus) {
        self.mining_stats = None;
        let log = miner_recovery::read_log_tail(
            &PathBuf::from(&self.settings.data_dir).join("genoryn-miner.log"),
        );
        let diagnosis = miner_recovery::diagnose(
            &log,
            &status.to_string(),
            status.code(),
            self.miner_attempt,
            &self.settings.rpc_url,
        );
        if let Some(next) = diagnosis.retry {
            self.miner_attempt = next;
            self.miner_retry_at = Some(Instant::now() + MINER_RETRY_DELAY);
        }
        self.status = diagnosis.message.clone();
        self.miner_note = Some(diagnosis.message);
    }
    /// Starts the corrected run once its short delay has passed and mining is
    /// still possible.
    pub(super) fn miner_retry_tick(&mut self) {
        let Some(at) = self.miner_retry_at else {
            return;
        };
        if Instant::now() < at
            || self.process_busy
            || self.miner.is_some()
            || self.network_transition.is_some()
        {
            return;
        }
        self.miner_retry_at = None;
        if self.address.is_empty() || self.node_info.is_none() {
            self.miner_note = Some(
                "Automatic miner restart skipped: the node is not reachable or no mining address is selected."
                    .into(),
            );
            return;
        }
        self.start_process("miner");
    }
    pub(super) fn check_for_updates(&mut self) {
        if self.update_status == updates::UpdateStatus::Checking {
            return;
        }
        self.update_status = updates::UpdateStatus::Checking;
        let tx = self.tx.clone();
        let lite = self.settings.lite;
        let route = network_privacy::auxiliary_route(&self.preferences, &self.settings);
        thread::spawn(move || {
            let result = route
                .and_then(|route| updates::fetch_latest(lite, route))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::Update(result));
        });
    }
    /// Keeps the balance current without a manual scan: rescans in the background
    /// once the node reports a block the wallet has not scanned. Slow scans (a first
    /// scan, or a remote node) widen the pause so scanning stays a minority of the time.
    pub(super) fn auto_scan_interval(&self) -> Duration {
        AUTO_SCAN_INTERVAL.max(self.last_scan_duration * 3)
    }
    pub(super) fn auto_scan(&mut self) {
        let Some(node_height) = self.node_info.as_ref().map(|node| node.info.height) else {
            return;
        };
        if !self.unlocked
            || self.scan_busy.is_some()
            || !self.seed_backup.is_empty()
            || self.preview.is_some()
            || self.last_auto_scan.elapsed() < self.auto_scan_interval()
        {
            return;
        }
        if self.wallet_height.is_none_or(|height| height < node_height) {
            self.last_auto_scan = Instant::now();
            self.request("sync", json!({}));
            self.auto_scan_running = self.scan_busy.is_some();
        } else if self.pending_password.is_none()
            && self.last_batch_scan.elapsed() >= AUTO_SCAN_INTERVAL.max(self.batch_scan_duration)
        {
            // Other open wallets catch up in batches, never ahead of unlocking.
            self.last_batch_scan = Instant::now();
            self.scan_sessions_batch(node_height);
        }
    }
    /// Decided once, on the first health result after launch: an already-running
    /// node is used as is, and a node stopped later is never silently restarted.
    pub(super) fn auto_start_node(&mut self, reachable: bool) {
        if self.auto_node_checked {
            return;
        }
        self.auto_node_checked = true;
        if reachable
            || !self.settings.auto_node
            || self.preferences.light_wallet
            || self.settings.smoke_test_frames.is_some()
            || self.node.is_some()
        {
            return;
        }
        self.start_process("node");
    }
    pub(super) fn start_process(&mut self, kind: &'static str) {
        if self.process_busy
            || (kind == "node" && self.node.is_some())
            || (kind == "miner" && self.miner.is_some())
        {
            return;
        }
        let privacy_arguments = if kind == "node" {
            match network_privacy::node_arguments(&self.preferences) {
                Ok(arguments) => arguments,
                Err(error) => {
                    self.status = format!("Node not started: {error}");
                    return;
                }
            }
        } else {
            Vec::new()
        };
        self.process_busy = true;
        let settings = self.settings.clone();
        // Automatic restarts keep paying the wallet chosen when mining started.
        let address = match (&self.miner_destination, kind) {
            (Some((address, _)), "miner") => address.clone(),
            _ => self.address.clone(),
        };
        let attempt = self.miner_attempt;
        let tx = self.tx.clone();
        self.process_worker = Some(thread::spawn(move || {
            let result = (|| -> Result<OwnedChild> {
                std::fs::create_dir_all(&settings.data_dir)?;
                let name = if kind == "node" {
                    "genorynd"
                } else {
                    "genoryn-miner"
                };
                let binary = protocol::sibling_binary(name, &settings.bin_dir)?;
                let log = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(PathBuf::from(&settings.data_dir).join(format!("{name}.log")))?;
                if kind == "miner" {
                    use std::io::Write as _;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |elapsed| elapsed.as_secs());
                    writeln!(&log, "{}", attempt.header(now))?;
                }
                let mut command = protocol::gui_child_command(&binary);
                command
                    .stdin(Stdio::null())
                    .stderr(Stdio::from(log.try_clone()?));
                if kind == "node" {
                    let url =
                        genoryn_connect::validate_endpoint(&settings.rpc_url, NETWORK.name())?;
                    let host = url
                        .host_str()
                        .context("RPC host required")?
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .parse::<std::net::IpAddr>()?;
                    ensure!(host.is_loopback(), "managed node must bind a loopback IP");
                    let rpc = std::net::SocketAddr::new(
                        host,
                        url.port_or_known_default()
                            .unwrap_or(NETWORK.base_port() + 1),
                    );
                    command
                        .args([
                            "--network",
                            NETWORK.name(),
                            "--data-dir",
                            &settings.data_dir,
                            "--rpc-listen",
                            &rpc.to_string(),
                        ])
                        .args(&privacy_arguments)
                        .stdout(Stdio::from(log));
                    if !settings.node_config.trim().is_empty() {
                        command.args(["--config", settings.node_config.trim()]);
                    }
                } else {
                    ensure!(
                        !address.is_empty(),
                        "unlock a wallet and confirm a mining destination first"
                    );
                    let token = PathBuf::from(&settings.data_dir).join("admin.token");
                    ensure!(token.is_file(), "local node admin token is missing");
                    command
                        .args([
                            "--network",
                            NETWORK.name(),
                            "--node",
                            &settings.rpc_url,
                            "--token-file",
                            &token.display().to_string(),
                            "--wallet",
                            &address,
                            "--threads",
                            &attempt.threads.to_string(),
                        ])
                        .stdout(Stdio::piped());
                    if attempt.portable {
                        command.env("GENORYN_RANDOMX_PORTABLE", "1");
                    }
                }
                let mut child = command.spawn().map_err(|error| {
                    anyhow::anyhow!(miner_recovery::spawn_failure(&binary, &error))
                })?;
                if kind == "miner"
                    && let Some(stdout) = child.stdout.take()
                {
                    let tx = tx.clone();
                    thread::spawn(move || {
                        let mut reader = BufReader::new(stdout);
                        loop {
                            let mut line = String::new();
                            match reader.by_ref().take(16_385).read_line(&mut line) {
                                Ok(0) | Err(_) => break,
                                Ok(_) if line.len() > 16_384 => break,
                                Ok(_) => {
                                    if let Ok(value) = serde_json::from_str::<Value>(&line)
                                        && value["event"].as_str() == Some("mining")
                                    {
                                        let _ = tx.send(Event::Mining(value));
                                    }
                                }
                            }
                        }
                    });
                }
                Ok(OwnedChild::new(child))
            })()
            .map_err(|e| e.to_string());
            let _ = tx.send(Event::Started { kind, result });
        }));
    }
    pub(super) fn stop_process(&mut self, kind: &'static str) {
        if self.process_busy {
            return;
        }
        let child = if kind == "node" {
            self.node.take()
        } else {
            self.miner.take()
        };
        if let Some(child) = child {
            self.process_busy = true;
            let tx = self.tx.clone();
            self.process_worker = Some(thread::spawn(move || {
                let result = protocol::stop_owned_child(child).map_err(|e| e.to_string());
                let _ = tx.send(Event::Stopped { kind, result });
            }));
        }
    }
}
