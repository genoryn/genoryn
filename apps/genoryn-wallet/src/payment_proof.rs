//! Selective disclosure UI. Cryptographic and chain verification stay in the paired CLI.
use super::*;
use genoryn_core::codec::MAX_ACTIONS;
use genoryn_wallet_core::{keys::decode_address, scan::NoteProof};
use std::{io::Write, path::Path};

pub(super) const MAX_PROOF_BYTES: usize = 16 * 1024;

/// Note openings are privacy-sensitive even though they cannot authorize spending.
pub(super) struct Proof(NoteProof);
impl Drop for Proof {
    fn drop(&mut self) {
        self.0.txid.zeroize();
        self.0.recipient.zeroize();
        self.0.amount.zeroize();
        self.0.rho.zeroize();
        self.0.rseed.zeroize();
        self.0.cmx.zeroize();
    }
}
impl Proof {
    fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_PROOF_BYTES,
            "Payment proof exceeds the 16 KiB limit."
        );
        // Parse the shared format; never interpret input as a URL or a command.
        let proof = Self(serde_json::from_str::<NoteProof>(text).map_err(|_| {
            anyhow::anyhow!("Invalid payment-proof JSON or missing/invalid fields.")
        })?);
        ensure!(proof.0.version == 1, "Unsupported payment-proof version.");
        ensure!(
            proof.0.network == NETWORK,
            "Payment proof is for a different network; select a mainnet proof."
        );
        ensure!(
            valid_txid(&proof.0.txid),
            "Payment proof requires a canonical transaction ID."
        );
        ensure!(
            (proof.0.action as usize) < MAX_ACTIONS,
            "Payment-proof output index exceeds the protocol limit."
        );
        decode_address(NETWORK, &proof.0.recipient)
            .map_err(|_| anyhow::anyhow!("Payment proof has an invalid recipient."))?;
        Ok(proof)
    }
    fn text(&self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(serde_json::to_string_pretty(&self.0)?))
    }
    fn value(&self) -> Result<Value> {
        Ok(serde_json::to_value(&self.0)?)
    }
}
fn valid_txid(txid: &str) -> bool {
    txid.len() == 64
        && txid
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[derive(Default)]
pub(super) struct ProofUi {
    selection: Option<(String, u32)>,
    prepared: Option<Proof>,
    acknowledged: bool,
    pub(super) saved: bool,
    pub(super) input: Zeroizing<String>,
    file_path: String,
    pub(super) verified: Option<SecretJson>,
}
pub(super) enum FileResult {
    Loaded(Zeroizing<String>),
    Saved(PathBuf),
}
fn read_proof_file(path: &Path, cancelled: &AtomicBool) -> Result<Zeroizing<String>> {
    ensure!(
        !cancelled.load(Ordering::Acquire),
        "Proof import cancelled."
    );
    ensure!(!path.as_os_str().is_empty(), "Choose a local proof file.");
    let name = path.to_string_lossy();
    ensure!(
        !name.contains("://") && !name.starts_with("//") && !name.starts_with("\\\\"),
        "Use a local file path, not a URL or network share."
    );
    let metadata =
        std::fs::symlink_metadata(path).context("Cannot inspect the local proof file.")?;
    ensure!(
        metadata.file_type().is_file(),
        "Choose a regular local file, not a directory, link or device."
    );
    ensure!(
        metadata.len() <= MAX_PROOF_BYTES as u64,
        "Payment proof exceeds the 16 KiB limit."
    );
    let file = std::fs::File::open(path).context("Cannot open the local proof file.")?;
    ensure!(
        file.metadata()?.is_file(),
        "Choose a regular local proof file."
    );
    let mut text = Zeroizing::new(String::new());
    file.take((MAX_PROOF_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .context("Cannot read proof as UTF-8 text.")?;
    let proof = Proof::parse(&text)?;
    ensure!(
        !cancelled.load(Ordering::Acquire),
        "Proof import cancelled."
    );
    proof.text()
}
fn proof_destination(data_dir: &str, proof: &NoteProof) -> PathBuf {
    PathBuf::from(data_dir)
        .join("payment-proofs")
        .join(format!("{}-{}.json", proof.txid, proof.action))
}
fn save_proof_file(path: &Path, text: &str, cancelled: &AtomicBool) -> Result<()> {
    ensure!(!cancelled.load(Ordering::Acquire), "Proof save cancelled.");
    let parent = path.parent().context("Proof directory unavailable.")?;
    match std::fs::create_dir(parent) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e).context("Cannot create the local payment-proofs directory."),
    }
    ensure!(
        std::fs::symlink_metadata(parent)?.file_type().is_dir(),
        "Proof directory must be a local directory, not a link."
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    ensure!(!cancelled.load(Ordering::Acquire), "Proof save cancelled.");
    let mut file = options.open(path).context(
        "Cannot save proof: the file may already exist. Existing files are never overwritten.",
    )?;
    file.write_all(text.as_bytes()).and_then(|_| file.sync_all())
        .context("Proof save did not finish; a partial local file may remain. Inspect it before retrying.")?;
    Ok(())
}

impl WalletApp {
    pub(super) fn select_proof(&mut self, txid: &str) {
        if self.busy.is_some() || !self.unlocked || !valid_txid(txid) {
            return;
        }
        self.proofs.selection = Some((txid.into(), 0));
        self.proofs.prepared = None;
        self.proofs.acknowledged = false;
        self.proofs.saved = false;
    }
    fn clear_proofs(&mut self) {
        if matches!(
            self.busy.as_deref(),
            Some("export_proof" | "verify_proof" | "import_proof" | "save_proof")
        ) {
            self.cancellation.store(true, Ordering::Release);
            self.generation = self.generation.wrapping_add(1);
        }
        self.proofs = ProofUi::default();
        self.status = "Proof view cleared. A local save already started may have completed; clipboard copies remain outside the wallet.".into();
    }
    pub(super) fn accept_exported_proof(&mut self, value: &Value) {
        let result = (|| -> Result<Proof> {
            ensure!(self.unlocked, "Unlock the wallet to prepare a proof.");
            let text = Zeroizing::new(serde_json::to_string(value)?);
            let proof = Proof::parse(&text)?;
            ensure!(
                self.proofs.selection.as_ref().is_some_and(
                    |(txid, action)| *txid == proof.0.txid && *action == proof.0.action
                ),
                "Proof result does not match the selected output."
            );
            Ok(proof)
        })();
        match result {
            Ok(proof) => {
                self.proofs.prepared = Some(proof);
                self.proofs.acknowledged = false;
                self.proofs.saved = false;
                self.status =
                    "Proof prepared locally. Review the exact disclosure before copying or saving."
                        .into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    pub(super) fn accept_verified_proof(&mut self, value: &Value) {
        let result = (|| -> Result<SecretJson> {
            let proof = Proof::parse(&self.proofs.input)?;
            ensure!(
                value["recipient"].as_str() == Some(proof.0.recipient.as_str())
                    && value["amount_atomic"].as_str() == Some(proof.0.amount.to_string().as_str()),
                "Verification response does not match the imported proof."
            );
            Ok(SecretJson(
                json!({"txid":proof.0.txid,"action":proof.0.action,
                "recipient":proof.0.recipient,"amount_atomic":proof.0.amount.to_string(),
                "endpoint":self.settings.rpc_url}),
            ))
        })();
        match result {
            Ok(result) => {
                self.proofs.verified = Some(result);
                self.status = "Payment proof verified against the selected validated chain. Reorganizations remain possible.".into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    fn proof_file_job(&mut self, op: &'static str, path: PathBuf, text: Option<Zeroizing<String>>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(op.into());
        self.cancellation = Arc::new(AtomicBool::new(false));
        let cancelled = self.cancellation.clone();
        let generation = self.generation;
        let tx = self.tx.clone();
        self.wallet_worker = Some(thread::spawn(move || {
            let result = if let Some(text) = text {
                save_proof_file(&path, &text, &cancelled).map(|_| FileResult::Saved(path))
            } else {
                read_proof_file(&path, &cancelled).map(FileResult::Loaded)
            }
            .map_err(|e| e.to_string());
            let _ = tx.send(Event::ProofFile { generation, result });
        }));
    }
    pub(super) fn proof_verifier(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Verify payment proof (no unlock needed)")
            .default_open(!self.unlocked).show(ui, |ui| {
            ui.label("Paste proof JSON or load a local file (16 KiB maximum). Verification uses the configured node; proof contents are checked locally by the paired CLI.");
            let response = egui::ScrollArea::vertical().id_salt("proof_json_scroll")
                .max_height(130.0).auto_shrink([false, true]).show(ui, |ui| {
                    protocol::sensitive_edit(ui, self.busy.is_none(),
                        area(&mut *self.proofs.input)
                            .id_salt("payment_proof_input").desired_rows(4).desired_width(f32::INFINITY)
                            .char_limit(MAX_PROOF_BYTES + 1).hint_text("Payment-proof JSON"))
                }).inner;
            if response.changed() { self.proofs.verified = None; }
            ui.horizontal_wrapped(|ui| {
                ui.label("Local file");
                protocol::sensitive_edit(ui, self.busy.is_none(), field(&mut self.proofs.file_path)
                    .desired_width(420.0).char_limit(4096).hint_text("Path to a .json proof"));
                if ui.add_enabled(self.busy.is_none() && !self.proofs.file_path.is_empty(), egui::Button::new("Load proof file")).clicked() {
                    self.proofs.verified = None;
                    self.proofs.input.zeroize();
                    self.proof_file_job("import_proof", PathBuf::from(&self.proofs.file_path), None);
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.busy.is_none() && !self.proofs.input.is_empty(), egui::Button::new("Verify proof")).clicked() {
                    self.proofs.verified = None;
                    match Proof::parse(&self.proofs.input).and_then(|p| p.value()) {
                        Ok(value) => self.request("verify_proof", json!({"proof":value})),
                        Err(error) => self.status = error.to_string(),
                    }
                }
                if ui.button("Clear / cancel proof").clicked() { self.clear_proofs(); }
            });
            if let Some(result) = &self.proofs.verified {
                ui.separator();
                ui.label(RichText::new("VERIFIED PAYMENT OUTPUT").strong());
                ui.label(format!("{} GNR · mainnet · output {}", format_atomic(result.0["amount_atomic"].as_str().unwrap_or_default()), result.0["action"]));
                ui.label(format!("Recipient: {}", result.0["recipient"].as_str().unwrap_or_default()));
                ui.label(format!("Transaction: {}", result.0["txid"].as_str().unwrap_or_default()));
                ui.label(format!("Selected node: {}", result.0["endpoint"].as_str().unwrap_or_default()));
                ui.label("Proves this output's recipient and amount in the selected chain at verification time. It does not establish sender identity, unspent status or finality. Memo is not included.");
            }
        });
    }
    pub(super) fn proof_export_window(&mut self, ctx: &egui::Context) {
        let Some((txid, mut action)) = self.proofs.selection.clone() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        egui::Window::new("Review selective payment proof").open(&mut open)
            .collapsible(false).resizable(true).default_width(540.0).default_height(560.0)
            .max_width(650.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .vscroll(true).show(ctx, |ui| {
            ui.label(format!("Transaction: {txid}"));
            ui.label("Select one output (zero-based). A transaction can contain both recipient and change outputs. Prepare the proof to inspect the exact recipient and amount.");
            ui.horizontal_wrapped(|ui| {
                ui.label("Output index");
                if ui.add_enabled(self.busy.is_none(), egui::DragValue::new(&mut action).range(0..=(MAX_ACTIONS - 1) as u32)).changed() {
                    self.proofs.selection = Some((txid.clone(), action));
                    self.proofs.prepared = None;
                    self.proofs.acknowledged = false;
                    self.proofs.saved = false;
                }
                if ui.add_enabled(self.busy.is_none(), egui::Button::new("Prepare selected output")).clicked() {
                    self.proofs.prepared = None;
                    self.proofs.acknowledged = false;
                    self.proofs.saved = false;
                    self.request("export_proof", json!({"txid":txid,"action":action}));
                }
            });
            if let Some(proof) = &self.proofs.prepared {
                ui.separator();
                ui.label(RichText::new("THIS PROOF REVEALS").strong());
                ui.label(format!("Recipient: {}", proof.0.recipient));
                ui.label(format!("Amount: {} GNR", format_atomic(&proof.0.amount.to_string())));
                ui.label(format!("Network: mainnet · Output index: {}", proof.0.action));
                ui.label("The transaction ID, output commitment (cmx), and note-opening data (rho and rseed) are included. Anyone holding this file can link this recipient and amount to this output. Memo, spending keys and other output openings are not included.");
                ui.add_space(8.0);
                ui.checkbox(&mut self.proofs.acknowledged, "I understand and approve disclosing this exact output.");
                let destination = proof_destination(&self.settings.data_dir, &proof.0);
                ui.label(format!("Local save destination: {}", destination.display()));
                ui.label("Saving never overwrites a file. Copying exposes the proof to clipboard readers and clipboard history. Nothing is automatically shared.");
                let mut copy = false;
                let mut save = false;
                ui.horizontal_wrapped(|ui| {
                    let enabled = self.proofs.acknowledged && self.busy.is_none();
                    copy = ui.add_enabled(enabled, egui::Button::new("Copy reviewed proof")).clicked();
                    save = ui.add_enabled(enabled && !self.proofs.saved, egui::Button::new("Save reviewed proof")).clicked();
                });
                if copy || save {
                    match proof.text() {
                        Ok(text) if copy => {
                            ctx.copy_text(text.to_string());
                            self.clipboard_warning = Some(Instant::now());
                            self.status = "Reviewed proof copied. Clipboard readers can see the disclosed recipient, amount and output data.".into();
                        }
                        Ok(text) => self.proof_file_job("save_proof", destination, Some(text)),
                        Err(error) => self.status = error.to_string(),
                    }
                }
                if self.proofs.saved { ui.label("Proof saved locally."); }
            }
            if ui.button("Close / cancel").clicked() { cancel = true; }
        });
        if !open || cancel {
            self.clear_proofs();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn proof_json() -> String {
        let wallet = genoryn_wallet_core::keys::Wallet::create(NETWORK).unwrap();
        serde_json::to_string(&NoteProof {
            version: 1,
            network: NETWORK,
            txid: "ab".repeat(32),
            action: 0,
            recipient: wallet.receiving_identity().unwrap(),
            amount: 1,
            rho: [1; 32],
            rseed: [2; 32],
            cmx: [3; 32],
        })
        .unwrap()
    }
    #[test]
    fn malformed_and_oversized_proofs_fail_before_any_cli_call() {
        let text = proof_json();
        assert!(Proof::parse(&text).is_ok());
        for malformed in [
            "".into(),
            "https://example.com/proof".into(),
            "{}".into(),
            "[".repeat(MAX_PROOF_BYTES + 1),
        ] {
            assert!(Proof::parse(&malformed).is_err());
        }
        for (key, bad) in [
            ("network", json!("devnet")),
            ("version", json!(2)),
            ("txid", json!("../other")),
            ("action", json!(MAX_ACTIONS)),
            ("rho", json!([1])),
            ("recipient", json!("not-an-address")),
        ] {
            let mut value: Value = serde_json::from_str(&text).unwrap();
            value[key] = bad;
            assert!(Proof::parse(&value.to_string()).is_err(), "{key}");
        }
    }
    #[test]
    fn local_export_never_overwrites_and_import_is_bounded() {
        let directory = std::env::temp_dir().join(format!(
            "genoryn-proof-test-{}-{}",
            std::process::id(),
            genoryn_core::chain::unix_time()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let result = (|| -> Result<()> {
            let cancelled = AtomicBool::new(false);
            let text = Zeroizing::new(proof_json());
            let proof = Proof::parse(&text)?;
            let path = proof_destination(directory.to_str().unwrap(), &proof.0);
            save_proof_file(&path, &text, &cancelled)?;
            assert!(save_proof_file(&path, "overwrite", &cancelled).is_err());
            assert_eq!(std::fs::read_to_string(&path)?, text.as_str());
            assert!(Proof::parse(&read_proof_file(&path, &cancelled)?).is_ok());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path)?.permissions().mode() & 0o777,
                    0o600
                );
                let link = directory.join("proof-link.json");
                std::os::unix::fs::symlink(&path, &link)?;
                assert!(read_proof_file(&link, &cancelled).is_err());
            }
            let oversized = directory.join("oversized.json");
            std::fs::write(&oversized, vec![b' '; MAX_PROOF_BYTES + 1])?;
            assert!(read_proof_file(&oversized, &cancelled).is_err());
            assert!(read_proof_file(&directory, &cancelled).is_err());
            for name in [
                "https://example.com/proof",
                "//host/share",
                "\\\\host\\share",
            ] {
                assert!(read_proof_file(Path::new(name), &cancelled).is_err());
            }
            cancelled.store(true, Ordering::Release);
            assert!(read_proof_file(&path, &cancelled).is_err());
            Ok(())
        })();
        std::fs::remove_dir_all(directory).unwrap();
        result.unwrap();
    }
    #[test]
    fn clear_lock_and_late_results_cannot_restore_disclosures() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.unlocked = true;
        let text = proof_json();
        let proof = Proof::parse(&text).unwrap();
        app.select_proof(&proof.0.txid);
        app.accept_exported_proof(&proof.value().unwrap());
        assert!(app.proofs.prepared.is_some());
        assert!(!app.proofs.acknowledged);
        app.proofs.acknowledged = true;
        app.proofs.input = Zeroizing::new(text);
        app.busy = Some("verify_proof".into());
        let generation = app.generation;
        app.clear_proofs();
        assert!(app.proofs.prepared.is_none());
        assert!(!app.proofs.acknowledged);
        assert!(app.proofs.input.is_empty());
        app.tx
            .send(Event::Wallet {
                generation,
                op: "export_proof".into(),
                result: Ok(SecretJson(proof.value().unwrap())),
            })
            .unwrap();
        app.poll();
        assert!(app.proofs.prepared.is_none());
        assert!(app.busy.is_none());
        app.select_proof(&proof.0.txid);
        app.accept_exported_proof(&proof.value().unwrap());
        app.lock();
        assert!(app.proofs.prepared.is_none());
        assert!(app.proofs.selection.is_none());
        assert!(app.proofs.verified.is_none());
    }
    #[test]
    fn wrong_output_results_fail_and_selection_is_frozen_while_busy() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.unlocked = true;
        let proof = Proof::parse(&proof_json()).unwrap();
        app.select_proof(&"cd".repeat(32));
        app.accept_exported_proof(&proof.value().unwrap());
        assert!(app.proofs.prepared.is_none());
        app.busy = Some("export_proof".into());
        app.select_proof(&proof.0.txid);
        assert_eq!(app.proofs.selection.as_ref().unwrap().0, "cd".repeat(32));
    }
    #[test]
    fn populated_proof_views_render_in_both_appearances() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.unlocked = true;
        let text = proof_json();
        let proof = Proof::parse(&text).unwrap();
        app.select_proof(&proof.0.txid);
        app.accept_exported_proof(&proof.value().unwrap());
        app.proofs.input = Zeroizing::new(text);
        app.accept_verified_proof(&json!({"recipient":proof.0.recipient,"amount_atomic":"1"}));
        assert!(app.proofs.verified.is_some());
        for light in [false, true] {
            set_style(&ctx, light);
            for size in [egui::vec2(900.0, 680.0), egui::vec2(1180.0, 820.0)] {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| app.proof_verifier(ui));
                        app.proof_export_window(ctx);
                    },
                );
                for primitive in ctx.tessellate(output.shapes, output.pixels_per_point) {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        for vertex in mesh.vertices {
                            assert_eq!(vertex.color.r(), vertex.color.g());
                            assert_eq!(vertex.color.g(), vertex.color.b());
                        }
                    }
                }
            }
        }
    }
}
