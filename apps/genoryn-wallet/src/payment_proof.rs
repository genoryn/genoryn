//! Payment proofs: made in one step after a payment, shared as a text code, a
//! JSON file or a PDF, and checked on the Verify payment page. Cryptographic and
//! chain verification stay in the paired CLI.
use super::*;
use genoryn_core::codec::MAX_ACTIONS;
use genoryn_wallet_core::{keys::decode_address, scan::NoteProof};
use proof_formats::{MAX_PROOF_FILE_BYTES, PdfDetails};
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
        if let Some(key) = &mut self.0.sender_key {
            key.zeroize();
        }
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
    /// Any shape a person may hand over: JSON, the text code, or a PDF's bytes.
    fn from_any(input: &[u8]) -> Result<Self> {
        Self::parse(&proof_formats::extract_proof_json(input)?)
    }
    fn text(&self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(serde_json::to_string_pretty(&self.0)?))
    }
    fn code(&self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(proof_formats::encode_code(&self.text()?)?))
    }
    fn value(&self) -> Result<Value> {
        Ok(serde_json::to_value(&self.0)?)
    }
    fn pdf(&self) -> Result<Vec<u8>> {
        let amount = grouped_amount(&self.0.amount.to_string());
        Ok(proof_formats::pdf_document(
            &PdfDetails {
                amount: &amount,
                recipient: &self.0.recipient,
                txid: &self.0.txid,
                action: self.0.action,
                by_sender: self.0.sender_key.is_some(),
                created_utc: proof_formats::utc_text(genoryn_core::chain::unix_time()),
            },
            &self.code()?,
        ))
    }
    fn file_stem(&self) -> String {
        format!("genoryn-payment-proof-{}", &self.0.txid[..12])
    }
}
fn valid_txid(txid: &str) -> bool {
    txid.len() == 64
        && txid
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaveKind {
    Json,
    Pdf,
}
impl SaveKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Default)]
pub(super) struct ProofUi {
    /// Transaction and output to prove; no output means the payment's own output.
    selection: Option<(String, Option<u32>)>,
    prepared: Option<Proof>,
    /// Whether the prepared proof's transaction is already in a block.
    mined: bool,
    pub(super) saved: Option<PathBuf>,
    pub(super) input: Zeroizing<String>,
    pub(super) expected: String,
    /// Without a file dialog (Linux), proofs are loaded by path.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    file_path: String,
    pub(super) verified: Option<SecretJson>,
    pub(super) verify_error: Option<String>,
}
pub(super) enum FileResult {
    Loaded(Zeroizing<String>),
    Saved(PathBuf),
}
/// Reads a local proof file of any supported shape and returns its JSON.
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
        metadata.len() <= MAX_PROOF_FILE_BYTES as u64,
        "This file is too large to be a payment proof."
    );
    let file = std::fs::File::open(path).context("Cannot open the local proof file.")?;
    ensure!(
        file.metadata()?.is_file(),
        "Choose a regular local proof file."
    );
    let mut bytes = Vec::new();
    file.take((MAX_PROOF_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("Cannot read the proof file.")?;
    let proof = Proof::from_any(&bytes);
    bytes.zeroize();
    let proof = proof?;
    ensure!(
        !cancelled.load(Ordering::Acquire),
        "Proof import cancelled."
    );
    proof.text()
}
/// Where proofs are saved when no save dialog is available: the user's
/// Documents folder, else the wallet's data folder.
fn default_proof_folder(data_dir: &str) -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|home| PathBuf::from(home).join("Documents"))
        .filter(|documents| documents.is_dir())
        .map(|documents| documents.join("GENORYN payment proofs"))
        .unwrap_or_else(|| PathBuf::from(data_dir).join("payment-proofs"))
}
fn proof_destination(folder: &Path, stem: &str, kind: SaveKind) -> PathBuf {
    folder.join(format!("{stem}.{}", kind.extension()))
}
fn save_proof_file(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<()> {
    ensure!(!cancelled.load(Ordering::Acquire), "Proof save cancelled.");
    let parent = path.parent().context("Proof directory unavailable.")?;
    match std::fs::create_dir_all(parent) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e).context("Cannot create the folder for payment proofs."),
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
        "Cannot save the proof: a file with this name already exists. Existing files are never overwritten.",
    )?;
    file.write_all(bytes).and_then(|_| file.sync_all())
        .context("Proof save did not finish; a partial local file may remain. Inspect it before retrying.")?;
    Ok(())
}
#[cfg(target_os = "windows")]
fn pick_save_path(suggested: &Path, kind: SaveKind) -> Option<PathBuf> {
    let label = match kind {
        SaveKind::Json => "Payment proof (JSON)",
        SaveKind::Pdf => "Payment proof (PDF)",
    };
    let mut dialog = rfd::FileDialog::new()
        .set_title("Save payment proof")
        .add_filter(label, &[kind.extension()]);
    if let Some(name) = suggested.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    if let Some(folder) = suggested.parent() {
        let _ = std::fs::create_dir_all(folder);
        dialog = dialog.set_directory(folder);
    }
    dialog.save_file()
}
#[cfg(target_os = "windows")]
fn pick_proof_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open a payment proof")
        .add_filter("Payment proof", &["json", "pdf", "txt"])
        .add_filter("All files", &["*"])
        .pick_file()
}

/// A check mark or a cross in a circle, then the text; monochrome like the app.
fn verdict(ui: &mut egui::Ui, ok: bool, text: impl Into<RichText>) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        let ink = if ok {
            ui.visuals().strong_text_color()
        } else {
            ERROR
        };
        let stroke = Stroke::new(1.8, ink);
        let painter = ui.painter();
        painter.circle_stroke(rect.center(), 9.0, stroke);
        let at = |x: f32, y: f32| rect.min + egui::vec2(x, y);
        if ok {
            painter.line_segment([at(5.5, 10.5), at(8.8, 13.8)], stroke);
            painter.line_segment([at(8.8, 13.8), at(14.8, 6.8)], stroke);
        } else {
            painter.line_segment([at(6.5, 6.5), at(13.5, 13.5)], stroke);
            painter.line_segment([at(13.5, 6.5), at(6.5, 13.5)], stroke);
        }
        let text = text.into();
        ui.add(egui::Label::new(if ok { text } else { text.color(ERROR) }).wrap());
    });
}
/// Long identifiers wrap instead of widening the page.
fn mono(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).monospace().size(12.5)).wrap());
}

impl WalletApp {
    /// Opens the proof window for `txid` and prepares the payment's own output.
    pub(super) fn select_proof(&mut self, txid: &str) {
        if self.busy.is_some() || !self.unlocked || !valid_txid(txid) {
            return;
        }
        self.proofs.selection = Some((txid.into(), None));
        self.proofs.prepared = None;
        self.proofs.saved = None;
        self.request("export_proof", json!({"txid": txid}));
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
        self.status = "Proof view cleared. A save already started may have completed; clipboard copies remain outside the wallet.".into();
    }
    pub(super) fn accept_exported_proof(&mut self, value: &Value) {
        let result = (|| -> Result<Proof> {
            ensure!(self.unlocked, "Unlock the wallet to prepare a proof.");
            let mut value = value.clone();
            let mined = value
                .as_object_mut()
                .and_then(|object| object.remove("mined_at"))
                .is_some_and(|height| !height.is_null());
            let text = Zeroizing::new(serde_json::to_string(&value)?);
            protocol::wipe_json(&mut value);
            let proof = Proof::parse(&text)?;
            ensure!(
                self.proofs
                    .selection
                    .as_ref()
                    .is_some_and(|(txid, action)| {
                        *txid == proof.0.txid
                            && action.is_none_or(|action| action == proof.0.action)
                    }),
                "Proof result does not match the selected output."
            );
            self.proofs.mined = mined;
            Ok(proof)
        })();
        match result {
            Ok(proof) => {
                if let Some((_, action)) = &mut self.proofs.selection {
                    *action = Some(proof.0.action);
                }
                self.proofs.prepared = Some(proof);
                self.proofs.saved = None;
                self.status = "Payment proof ready. Copy the code or save it as a file.".into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    pub(super) fn accept_verified_proof(&mut self, value: &Value) {
        let result = (|| -> Result<SecretJson> {
            let proof = Proof::from_any(self.proofs.input.as_bytes())?;
            ensure!(
                value["recipient"].as_str() == Some(proof.0.recipient.as_str())
                    && value["amount_atomic"].as_str() == Some(proof.0.amount.to_string().as_str()),
                "Verification response does not match the imported proof."
            );
            let mut result = value.clone();
            result["endpoint"] = json!(self.settings.rpc_url);
            result["checked_wallet"] = json!(
                self.unlocked
                    .then(|| sessions::wallet_name(&self.settings.wallet_path))
            );
            Ok(SecretJson(result))
        })();
        match result {
            Ok(result) => {
                self.proofs.verified = Some(result);
                self.proofs.verify_error = None;
                self.status = "Payment proof checked against the chain.".into();
            }
            Err(error) => {
                self.proofs.verify_error = Some(error.to_string());
                self.status = error.to_string();
            }
        }
    }
    fn proof_file_job(&mut self, op: &'static str, path: PathBuf, bytes: Option<Vec<u8>>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(op.into());
        self.cancellation = Arc::new(AtomicBool::new(false));
        let cancelled = self.cancellation.clone();
        let generation = self.generation;
        let tx = self.tx.clone();
        self.wallet_worker = Some(thread::spawn(move || {
            let result = if let Some(mut bytes) = bytes {
                let saved =
                    save_proof_file(&path, &bytes, &cancelled).map(|_| FileResult::Saved(path));
                bytes.zeroize();
                saved
            } else {
                read_proof_file(&path, &cancelled).map(FileResult::Loaded)
            }
            .map_err(|e| e.to_string());
            let _ = tx.send(Event::ProofFile { generation, result });
        }));
    }
    fn load_proof_file(&mut self, path: PathBuf) {
        self.proofs.verified = None;
        self.proofs.verify_error = None;
        self.proofs.input.zeroize();
        self.proof_file_job("import_proof", path, None);
    }
    fn verify_input(&mut self) {
        self.proofs.verified = None;
        self.proofs.verify_error = None;
        let expected = self.proofs.expected.trim().to_owned();
        if !expected.is_empty() && decode_address(NETWORK, &expected).is_err() {
            self.proofs.verify_error =
                Some("The address to check is not a valid GENORYN mainnet address.".into());
            return;
        }
        match Proof::from_any(self.proofs.input.as_bytes()).and_then(|proof| proof.value()) {
            Ok(value) => self.request(
                "verify_proof",
                json!({"proof": value, "expected_recipient": expected}),
            ),
            Err(error) => self.proofs.verify_error = Some(error.to_string()),
        }
    }
    /// The Verify payment page.
    pub(super) fn verify_page(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Verify payment",
            "Check a payment proof someone gave you: is the payment real, and did it go to the right address?",
        );
        ui.add_space(18.0);
        let dropped = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        });
        if let Some(path) = dropped
            && self.busy.is_none()
        {
            self.load_proof_file(path);
        }
        let idle = self.busy.is_none();
        card(ui, |ui| {
            ui.label(RichText::new("1 · THE PROOF").small().color(GRAY));
            ui.label("Paste the proof code or JSON, choose the .pdf or .json file, or drop the file onto this window.");
            ui.add_space(6.0);
            let response = egui::ScrollArea::vertical()
                .id_salt("proof_input_scroll")
                .max_height(140.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    protocol::sensitive_edit(
                        ui,
                        idle,
                        area(&mut *self.proofs.input)
                            .id_salt("payment_proof_input")
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .char_limit(MAX_PROOF_BYTES + 1)
                            .hint_text("GNRPROOF1-…  or  { \"version\": 1, … }"),
                    )
                })
                .inner;
            if response.changed() {
                self.proofs.verified = None;
                self.proofs.verify_error = None;
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                #[cfg(target_os = "windows")]
                if ui
                    .add_enabled(idle, egui::Button::new("Choose proof file…"))
                    .clicked()
                    && let Some(path) = pick_proof_file()
                {
                    self.load_proof_file(path);
                }
                #[cfg(not(target_os = "windows"))]
                {
                    protocol::sensitive_edit(
                        ui,
                        idle,
                        field(&mut self.proofs.file_path)
                            .desired_width(360.0)
                            .char_limit(4096)
                            .hint_text("Path to a .pdf or .json proof"),
                    );
                    if ui
                        .add_enabled(
                            idle && !self.proofs.file_path.is_empty(),
                            egui::Button::new("Load file"),
                        )
                        .clicked()
                    {
                        let path = PathBuf::from(&self.proofs.file_path);
                        self.load_proof_file(path);
                    }
                }
                if !self.proofs.input.is_empty() && ui.button("Clear").clicked() {
                    self.clear_proofs();
                }
            });
            ui.add_space(14.0);
            ui.label(
                RichText::new("2 · ADDRESS TO CHECK (OPTIONAL)")
                    .small()
                    .color(GRAY),
            );
            ui.label("The address the payment should have gone to — for example the address you gave the sender.");
            if protocol::sensitive_edit(
                ui,
                idle,
                field(&mut self.proofs.expected)
                    .desired_width(f32::INFINITY)
                    .char_limit(256)
                    .hint_text("gnr1…"),
            )
            .changed()
            {
                self.proofs.verified = None;
            }
            if self.unlocked && ui.small_button("Use my receiving address").clicked() {
                self.proofs.expected = self.address.clone();
                self.proofs.verified = None;
            }
            ui.add_space(14.0);
            if ui
                .add_enabled(
                    idle && !self.proofs.input.trim().is_empty(),
                    primary(ui, "Verify payment").min_size(egui::vec2(200.0, 44.0)),
                )
                .clicked()
            {
                self.verify_input();
            }
            if self.busy.as_deref() == Some("verify_proof") {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the proof against the chain…");
                });
            }
        });
        ui.add_space(16.0);
        if let Some(error) = &self.proofs.verify_error {
            card(ui, |ui| {
                verdict(ui, false, RichText::new("Not verified").size(20.0).strong());
                ui.add_space(4.0);
                ui.label(error.as_str());
            });
        }
        if let Some(result) = &self.proofs.verified {
            let result = &result.0;
            card(ui, |ui| {
                verdict(
                    ui,
                    true,
                    RichText::new("Real payment, found on the chain")
                        .size(20.0)
                        .strong(),
                );
                ui.add_space(10.0);
                gnr_amount(ui, result["amount_atomic"].as_str(), 30.0, None);
                ui.add_space(8.0);
                ui.label(RichText::new("PAID TO").small().color(GRAY));
                mono(ui, result["recipient"].as_str().unwrap_or_default());
                ui.add_space(6.0);
                ui.label(RichText::new("TRANSACTION").small().color(GRAY));
                mono(ui, result["txid"].as_str().unwrap_or_default());
                ui.label(
                    RichText::new(format!(
                        "In block {} · {} confirmation(s) · output {}",
                        result["block_height"], result["confirmations"], result["action"]
                    ))
                    .color(GRAY),
                );
                if let Some(memo) = result["memo"].as_str().filter(|memo| !memo.is_empty()) {
                    ui.add_space(6.0);
                    ui.label(RichText::new("MEMO").small().color(GRAY));
                    ui.add(egui::Label::new(memo).wrap());
                }
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                match result["recipient_matches"].as_bool() {
                    Some(true) => verdict(
                        ui,
                        true,
                        "Yes — this payment went to the address you entered.",
                    ),
                    Some(false) => verdict(
                        ui,
                        false,
                        "No — this payment did NOT go to the address you entered.",
                    ),
                    None => {}
                }
                if let (Some(mine), Some(name)) = (
                    result["belongs_to_wallet"].as_bool(),
                    result["checked_wallet"].as_str(),
                ) {
                    if mine {
                        verdict(
                            ui,
                            true,
                            format!("This payment was received by your wallet {name}."),
                        );
                    } else {
                        ui.label(
                            RichText::new(format!("Not paid to your open wallet {name}."))
                                .color(GRAY),
                        );
                    }
                }
                if result["created_by_sender"].as_bool() == Some(true) {
                    verdict(
                        ui,
                        true,
                        "Made by the sender: the proof contains a key only the paying wallet has.",
                    );
                } else {
                    ui.label(
                        RichText::new(
                            "Made from the receiving side: it proves the payment, but not who created the proof.",
                        )
                        .color(GRAY),
                    );
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Payments are private: the sender's address is never shown. A very recent block can still be replaced; more confirmations mean more certainty.",
                    )
                    .small()
                    .color(GRAY),
                );
            });
        }
    }
    /// The window that turns a payment into a proof to share.
    pub(super) fn proof_export_window(&mut self, ctx: &egui::Context) {
        let Some((txid, action)) = self.proofs.selection.clone() else {
            return;
        };
        let mut open = true;
        let mut close = false;
        let mut save = None;
        egui::Window::new("Payment proof")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .max_width(680.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .vscroll(true)
            .show(ctx, |ui| {
                let idle = self.busy.is_none();
                match &self.proofs.prepared {
                    None => {
                        ui.horizontal(|ui| {
                            if !idle {
                                ui.spinner();
                            }
                            ui.label(if idle {
                                "The proof could not be prepared; see the status line below."
                            } else {
                                "Preparing the proof…"
                            });
                        });
                    }
                    Some(proof) => {
                        ui.label(RichText::new("AMOUNT").small().color(GRAY));
                        gnr_amount(ui, Some(&proof.0.amount.to_string()), 26.0, None);
                        ui.add_space(6.0);
                        ui.label(RichText::new("PAID TO").small().color(GRAY));
                        mono(ui, &proof.0.recipient);
                        ui.add_space(6.0);
                        ui.label(RichText::new("TRANSACTION").small().color(GRAY));
                        mono(ui, &proof.0.txid);
                        ui.add_space(8.0);
                        if proof.0.sender_key.is_some() {
                            verdict(ui, true, "Made by your wallet as the sender — the recipient cannot create this proof.");
                        } else {
                            ui.label("This proof comes from the receiving side.");
                        }
                        if !self.proofs.mined {
                            ui.label(
                                RichText::new("The payment is still waiting for a block. The recipient can verify the proof once it is mined.")
                                    .color(GRAY),
                            );
                        }
                        ui.add_space(12.0);
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(idle, primary(ui, "Copy proof code")).clicked() {
                                match proof.code() {
                                    Ok(code) => {
                                        ctx.copy_text(code.to_string());
                                        self.clipboard_warning = Some(Instant::now());
                                        self.status = "Proof code copied. Paste it into a message to the person who should check the payment.".into();
                                    }
                                    Err(error) => self.status = error.to_string(),
                                }
                            }
                            if ui.add_enabled(idle, egui::Button::new("Save as PDF…")).clicked() {
                                save = Some(SaveKind::Pdf);
                            }
                            if ui.add_enabled(idle, egui::Button::new("Save as JSON…")).clicked() {
                                save = Some(SaveKind::Json);
                            }
                            if ui.add_enabled(idle, egui::Button::new("Copy JSON")).clicked() {
                                match proof.text() {
                                    Ok(text) => {
                                        ctx.copy_text(text.to_string());
                                        self.clipboard_warning = Some(Instant::now());
                                        self.status = "Proof JSON copied.".into();
                                    }
                                    Err(error) => self.status = error.to_string(),
                                }
                            }
                        });
                        if let Some(path) = &self.proofs.saved {
                            ui.label(format!("Saved: {}", path.display()));
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Whoever receives this proof sees this payment's recipient and amount (and its memo, if any). It can never move your coins.")
                                .small()
                                .color(GRAY),
                        );
                    }
                }
                ui.add_space(8.0);
                egui::CollapsingHeader::new("Advanced: prove another output of this transaction")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let mut index = action.unwrap_or(0);
                            ui.label("Output index");
                            ui.add_enabled(
                                idle,
                                egui::DragValue::new(&mut index).range(0..=(MAX_ACTIONS - 1) as u32),
                            );
                            if ui.add_enabled(idle, egui::Button::new("Prepare this output")).clicked() {
                                self.proofs.selection = Some((txid.clone(), Some(index)));
                                self.proofs.prepared = None;
                                self.proofs.saved = None;
                                self.request("export_proof", json!({"txid": txid, "action": index}));
                            } else if Some(index) != action && idle {
                                self.proofs.selection = Some((txid.clone(), Some(index)));
                            }
                        });
                    });
                ui.add_space(6.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        if let Some(kind) = save {
            self.save_prepared_proof(kind);
        }
        if !open || close {
            self.clear_proofs();
        }
    }
    fn save_prepared_proof(&mut self, kind: SaveKind) {
        let Some(proof) = &self.proofs.prepared else {
            return;
        };
        let suggested = proof_destination(
            &default_proof_folder(&self.settings.data_dir),
            &proof.file_stem(),
            kind,
        );
        #[cfg(target_os = "windows")]
        let path = match pick_save_path(&suggested, kind) {
            Some(path) => path,
            None => return,
        };
        #[cfg(not(target_os = "windows"))]
        let path = suggested;
        let bytes = match kind {
            SaveKind::Json => proof.text().map(|text| text.as_bytes().to_vec()),
            SaveKind::Pdf => proof.pdf(),
        };
        match bytes {
            Ok(bytes) => {
                // A save dialog may pick an existing file the user chose to replace.
                if cfg!(target_os = "windows") && path.is_file() {
                    let _ = std::fs::remove_file(&path);
                }
                self.proof_file_job("save_proof", path, Some(bytes));
            }
            Err(error) => self.status = error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn proof_json_with(sender_key: Option<[u8; 32]>) -> String {
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
            sender_key,
        })
        .unwrap()
    }
    fn proof_json() -> String {
        proof_json_with(None)
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
            assert!(Proof::from_any(malformed.as_bytes()).is_err());
        }
        for (key, bad) in [
            ("network", json!("devnet")),
            ("version", json!(2)),
            ("txid", json!("../other")),
            ("action", json!(MAX_ACTIONS)),
            ("rho", json!([1])),
            ("recipient", json!("not-an-address")),
            ("sender_key", json!([1, 2])),
        ] {
            let mut value: Value = serde_json::from_str(&text).unwrap();
            value[key] = bad;
            assert!(Proof::parse(&value.to_string()).is_err(), "{key}");
        }
    }
    #[test]
    fn every_shape_of_a_proof_reads_back_identically() {
        let text = proof_json_with(Some([9; 32]));
        let proof = Proof::parse(&text).unwrap();
        let code = proof.code().unwrap();
        let pdf = proof.pdf().unwrap();
        for input in [text.as_bytes(), code.as_bytes(), &pdf] {
            let again = Proof::from_any(input).unwrap();
            assert_eq!(again.value().unwrap(), proof.value().unwrap());
            assert_eq!(again.0.sender_key, Some([9; 32]));
        }
        assert!(
            proof
                .file_stem()
                .starts_with("genoryn-payment-proof-abababab")
        );
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
            let path = proof_destination(&directory, &proof.file_stem(), SaveKind::Json);
            save_proof_file(&path, text.as_bytes(), &cancelled)?;
            assert!(save_proof_file(&path, b"overwrite", &cancelled).is_err());
            assert_eq!(std::fs::read_to_string(&path)?, text.as_str());
            assert!(Proof::parse(&read_proof_file(&path, &cancelled)?).is_ok());
            let pdf_path = proof_destination(&directory, &proof.file_stem(), SaveKind::Pdf);
            save_proof_file(&pdf_path, &proof.pdf()?, &cancelled)?;
            assert_eq!(
                Proof::parse(&read_proof_file(&pdf_path, &cancelled)?)?.value()?,
                proof.value()?
            );
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
            std::fs::write(&oversized, vec![b' '; MAX_PROOF_FILE_BYTES + 1])?;
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
        app.proofs.selection = Some((proof.0.txid.clone(), None));
        app.accept_exported_proof(&proof.value().unwrap());
        assert!(app.proofs.prepared.is_some());
        app.proofs.input = Zeroizing::new(text);
        app.busy = Some("verify_proof".into());
        let generation = app.generation;
        app.clear_proofs();
        assert!(app.proofs.prepared.is_none());
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
        app.proofs.selection = Some((proof.0.txid.clone(), None));
        app.accept_exported_proof(&proof.value().unwrap());
        app.lock();
        assert!(app.proofs.prepared.is_none());
        assert!(app.proofs.selection.is_none());
        assert!(app.proofs.verified.is_none());
    }
    #[test]
    fn wrong_output_results_fail_and_the_payment_output_is_chosen_automatically() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.unlocked = true;
        let proof = Proof::parse(&proof_json()).unwrap();
        app.proofs.selection = Some(("cd".repeat(32), None));
        app.accept_exported_proof(&proof.value().unwrap());
        assert!(app.proofs.prepared.is_none(), "another transaction");
        app.proofs.selection = Some((proof.0.txid.clone(), Some(3)));
        app.accept_exported_proof(&proof.value().unwrap());
        assert!(app.proofs.prepared.is_none(), "another output");
        app.proofs.selection = Some((proof.0.txid.clone(), None));
        let mut value = proof.value().unwrap();
        value["mined_at"] = json!(12);
        app.accept_exported_proof(&value);
        assert!(app.proofs.prepared.is_some() && app.proofs.mined);
        assert_eq!(app.proofs.selection.as_ref().unwrap().1, Some(0));
        app.busy = Some("export_proof".into());
        app.select_proof(&"ef".repeat(32));
        assert_eq!(
            app.proofs.selection.as_ref().unwrap().0,
            proof.0.txid,
            "frozen while busy"
        );
    }
    #[test]
    fn an_invalid_address_to_check_is_caught_before_the_chain_is_asked() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.proofs.input = Zeroizing::new(proof_json());
        app.proofs.expected = "gnr1notanaddress".into();
        app.verify_input();
        assert!(
            app.proofs
                .verify_error
                .as_deref()
                .unwrap()
                .contains("not a valid")
        );
        app.proofs.expected.clear();
        app.proofs.input = Zeroizing::new("hello".into());
        app.verify_input();
        assert!(
            app.proofs
                .verify_error
                .as_deref()
                .unwrap()
                .contains("No payment proof found")
        );
    }
    #[test]
    fn populated_proof_views_render_in_both_appearances() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.unlocked = true;
        let text = proof_json_with(Some([4; 32]));
        let proof = Proof::parse(&text).unwrap();
        app.proofs.selection = Some((proof.0.txid.clone(), None));
        app.accept_exported_proof(&proof.value().unwrap());
        app.proofs.input = Zeroizing::new(text);
        app.accept_verified_proof(&json!({
            "recipient": proof.0.recipient, "amount_atomic": "1", "txid": proof.0.txid,
            "action": 0, "block_height": 9, "confirmations": 3, "memo": "invoice 7",
            "created_by_sender": true, "recipient_matches": true, "belongs_to_wallet": true,
        }));
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
                        egui::CentralPanel::default().show(ctx, |ui| app.verify_page(ui));
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
