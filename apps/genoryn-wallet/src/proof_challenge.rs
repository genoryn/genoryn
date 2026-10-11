//! Payment Proof 2.0 in the wallet: outcomes other than a verified payment,
//! the verifier's fresh sender challenge, and the payer's answer.
//!
//! The cryptography and the chain checks run in the paired CLI; this module
//! only carries codes between the person and the CLI and shows the results.
//! Nothing here is green unless the CLI verified it on the chain this wallet
//! validated.
use super::*;
use payment_proof::{mono, verdict};

const CHALLENGE_PREFIX: &str = "GNRCHAL1-";
const RESPONSE_PREFIX: &str = "GNRRESP1-";
const MAX_CODE: usize = 1_024;
/// Symbols after the prefix: base64url of 127 + 4 and 97 + 4 bytes.
const CHALLENGE_SYMBOLS: usize = 175;
const RESPONSE_SYMBOLS: usize = 135;

fn symbols(prefix: &str) -> usize {
    if prefix == CHALLENGE_PREFIX {
        CHALLENGE_SYMBOLS
    } else {
        RESPONSE_SYMBOLS
    }
}

/// Codes are pasted from messages, sometimes broken across lines: collect
/// exactly the code's own symbols after its prefix, skipping line breaks and
/// spaces, and nothing written after it.
fn clean_code(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let wanted = symbols(prefix);
    let mut code = String::from(prefix);
    for c in text[start..].chars() {
        if code.len() - prefix.len() == wanted {
            break;
        }
        if c.is_whitespace() {
            continue;
        }
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            break;
        }
        code.push(c);
    }
    (code.len() - prefix.len() == wanted).then_some(code)
}

impl WalletApp {
    pub(super) fn unconfirmed_outcome(&mut self, ui: &mut egui::Ui) {
        let Some(result) = &self.proofs.verified else {
            return;
        };
        let outcome = result.0["outcome"].as_str().unwrap_or_default().to_owned();
        let tip = result.0["chain_tip"]
            .as_u64()
            .or(result.0["last_verified_height"].as_u64());
        card(ui, |ui| match outcome.as_str() {
            "reorganized" => {
                verdict(
                    ui,
                    false,
                    RichText::new("Payment replaced or reorganized")
                        .size(20.0)
                        .strong(),
                );
                ui.label(format!(
                    "This payment was confirmed in block {} earlier in this session, but it is no longer on the chain this wallet validated (tip {}). Treat it as unpaid until it is confirmed again.",
                    result.0["previously_confirmed_at"], tip.unwrap_or_default()
                ));
            }
            "unavailable" => {
                ui.label(
                    RichText::new("Verification unavailable")
                        .size(20.0)
                        .strong(),
                );
                ui.label("No remote node answered, so this wallet has no current verified view of the chain. The proof was not checked; try again when a source is reachable.");
                if let Some(reason) = result.0["offline_reason"].as_str() {
                    ui.label(RichText::new(reason).small().color(GRAY));
                }
            }
            _ => {
                ui.label(
                    RichText::new("Payment pending or not found")
                        .size(20.0)
                        .strong(),
                );
                ui.label(format!(
                    "This transaction is not in any block of the chain this wallet validated (tip {}). If it was sent moments ago it may still be waiting for a block; check again after the next block. It does not count as paid yet.",
                    tip.unwrap_or_default()
                ));
            }
        });
    }

    pub(super) fn sender_verification_card(&mut self, ui: &mut egui::Ui) {
        let Some(result) = self.proofs.verified.as_ref().map(|r| r.0.clone()) else {
            return;
        };
        let idle = self.busy.is_none();
        ui.add_space(16.0);
        card(ui, |ui| {
            small_title(ui, "WHO SENT IT? (OPTIONAL)");
            match result["sender_evidence"].as_str() {
                Some("fresh_authentication_available") => {}
                Some("origin_disclosure_only") => {
                    ui.label(RichText::new("Legacy sender evidence only").strong());
                    ui.label("The proof was made with the paying wallet's key, but this payment was made without a sender-verification key (before GENORYN 1.1, or with it switched off). A fresh sender challenge is not possible for it.");
                    return;
                }
                _ => {
                    ui.label("This proof comes from the receiving side and the payment carries no sender-verification key this wallet can read, so the sender cannot be challenged with it. Ask the sender for their own proof.");
                    return;
                }
            }
            ui.label("Ask the person who says they paid to prove they still control this payment's key. Their wallet signs your fresh challenge; you learn nothing about their other payments or balance.");
            ui.add_space(8.0);
            match &self.proofs.challenge {
                None => {
                    if layout_probe::tag(
                        "verify:challenge",
                        ui.add_enabled(
                            idle,
                            primary(ui, "Request sender verification")
                                .min_size(egui::vec2(240.0, 44.0)),
                        ),
                    )
                    .clicked()
                    {
                        self.proofs.sender_result = None;
                        self.proofs.challenge_error = None;
                        self.request(
                            "create_payment_challenge",
                            json!({
                                "txid": result["txid"],
                                "action": result["action"],
                                "recipient": result["recipient"],
                                "amount_atomic": result["amount_atomic"],
                            }),
                        );
                    }
                    if let Some(error) = &self.proofs.challenge_error {
                        ui.label(RichText::new(error.as_str()).color(ERROR));
                    }
                }
                Some(challenge) => {
                    let challenge = challenge.clone();
                    ui.label(
                        RichText::new("1 · Send this challenge to the sender")
                            .small()
                            .color(GRAY),
                    );
                    mono(ui, &challenge);
                    ui.horizontal(|ui| {
                        if ui.button("Copy challenge").clicked() {
                            ui.ctx().copy_text(challenge.clone());
                            self.clipboard_warning = Some(Instant::now());
                        }
                        if ui
                            .add_enabled(idle, egui::Button::new("New challenge"))
                            .clicked()
                        {
                            self.proofs.challenge = None;
                            self.proofs.sender_result = None;
                        }
                    });
                    ui.label(RichText::new("It is valid for one hour and only for this payment, this address and this amount.").small().color(GRAY));
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("2 · Paste the sender's response")
                            .small()
                            .color(GRAY),
                    );
                    ui.add(
                        field(&mut self.proofs.response_input)
                            .hint_text("GNRRESP1-…")
                            .desired_width(f32::INFINITY)
                            .char_limit(MAX_CODE),
                    );
                    let response = clean_code(&self.proofs.response_input, RESPONSE_PREFIX);
                    if layout_probe::tag(
                        "verify:sender",
                        ui.add_enabled(
                            idle && response.is_some(),
                            primary(ui, "Verify sender response").min_size(egui::vec2(220.0, 44.0)),
                        ),
                    )
                    .clicked()
                    {
                        let proof = proof_value(&self.proofs.input);
                        self.request(
                            "verify_payment_challenge",
                            json!({"challenge": challenge, "response": response, "proof": proof}),
                        );
                    }
                }
            }
            if let Some(outcome) = &self.proofs.sender_result {
                ui.add_space(10.0);
                ui.separator();
                match outcome {
                    Ok(value) if value.0["outcome"] == "sender_verified" => {
                        verdict(
                            ui,
                            true,
                            RichText::new("Sender challenge verified")
                                .size(18.0)
                                .strong(),
                        );
                        ui.label(format!(
                            "{} GNR was sent to this address, and whoever answered holds the key this payment committed to when it was made. {} confirmation(s).",
                            format_atomic(value.0["amount_atomic"].as_str().unwrap_or("0")),
                            value.0["confirmations"]
                        ));
                        ui.label(RichText::new("This proves control of the payment's credential, not a legal identity. Link it to a person only through a channel you already trust; keys can be shared or stolen.").small().color(GRAY));
                    }
                    Ok(value) if value.0["outcome"] == "legacy_sender_evidence_only" => {
                        ui.label(RichText::new("Legacy sender evidence only").strong());
                        ui.label(value.0["note"].as_str().unwrap_or_default());
                    }
                    Ok(_) => {
                        verdict(ui, false, "Unexpected answer; nothing was verified.");
                    }
                    Err(error) => {
                        verdict(
                            ui,
                            false,
                            RichText::new("Sender verification failed").strong(),
                        );
                        ui.label(error.as_str());
                    }
                }
            }
        });
    }

    pub(super) fn answer_challenge_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            small_title(ui, "ANSWER A PAYMENT CHALLENGE");
            self.answer_challenge_controls(ui);
        });
    }
    pub(super) fn answer_challenge_controls(&mut self, ui: &mut egui::Ui) {
        let idle = self.busy.is_none();
        {
            ui.label("Someone you paid sent you a GNRCHAL1 code? Paste it here. Your wallet answers only if it made exactly that payment; the answer reveals no key, balance or other payment.");
            ui.add(
                field(&mut self.proofs.answer_input)
                    .hint_text("GNRCHAL1-…")
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_CODE),
            );
            let challenge = clean_code(&self.proofs.answer_input, CHALLENGE_PREFIX);
            if layout_probe::tag(
                "verify:answer",
                ui.add_enabled(
                    idle && self.unlocked && challenge.is_some(),
                    egui::Button::new("Answer challenge"),
                ),
            )
            .clicked()
            {
                self.proofs.answer_result = None;
                self.request("answer_payment_challenge", json!({"challenge": challenge}));
            }
            if !self.unlocked {
                ui.label(
                    RichText::new("Unlock the wallet that made the payment first.")
                        .small()
                        .color(GRAY),
                );
            }
            match &self.proofs.answer_result {
                Some(Ok(response)) => {
                    let response = response.clone();
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Send this response back:")
                            .small()
                            .color(GRAY),
                    );
                    mono(ui, &response);
                    if ui.button("Copy response").clicked() {
                        ui.ctx().copy_text(response);
                        self.clipboard_warning = Some(Instant::now());
                    }
                }
                Some(Err(error)) => {
                    ui.add_space(6.0);
                    ui.label(RichText::new(error.as_str()).color(ERROR));
                }
                None => {}
            }
        }
    }

    pub(super) fn accept_challenge(&mut self, value: &Value) {
        self.proofs.challenge = value["challenge"]
            .as_str()
            .and_then(|code| clean_code(code, CHALLENGE_PREFIX));
        self.proofs.response_input.clear();
        self.status = "Challenge ready. Send it to the sender and paste their response.".into();
    }
    pub(super) fn accept_answer(&mut self, value: &Value) {
        self.proofs.answer_result = Some(
            value["response"]
                .as_str()
                .and_then(|code| clean_code(code, RESPONSE_PREFIX))
                .ok_or_else(|| "The wallet returned no response code.".to_owned()),
        );
        self.status = "Response ready. Send it back to the person who asked.".into();
    }
}

/// The proof being verified, as the JSON the CLI checks again.
fn proof_value(input: &str) -> Option<Value> {
    proof_formats::extract_proof_json(input.as_bytes())
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_codes_are_cleaned_and_bounded() {
        let body = "aB-_9".repeat(35);
        let code = format!("{CHALLENGE_PREFIX}{body}");
        assert_eq!(
            clean_code(&format!("here: {code} thanks"), CHALLENGE_PREFIX).as_deref(),
            Some(code.as_str()),
            "words written after the code are not part of it"
        );
        let broken = format!("{}\n{} \r\n{}", &code[..40], &code[40..100], &code[100..]);
        assert_eq!(
            clean_code(&broken, CHALLENGE_PREFIX).as_deref(),
            Some(code.as_str()),
            "line breaks inside a pasted code are dropped"
        );
        assert_eq!(clean_code(&code[..120], CHALLENGE_PREFIX), None, "cut off");
        assert_eq!(clean_code("GNRCHAL1-", CHALLENGE_PREFIX), None);
        assert_eq!(clean_code("nothing", CHALLENGE_PREFIX), None);
        let response = format!("{RESPONSE_PREFIX}{}", "z".repeat(RESPONSE_SYMBOLS));
        assert_eq!(
            clean_code(&response, RESPONSE_PREFIX).as_deref(),
            Some(response.as_str())
        );
    }
    #[test]
    fn code_lengths_match_the_wallet_core_format() {
        let wallet = genoryn_wallet_core::keys::Wallet::create(NETWORK).unwrap();
        let challenge = genoryn_wallet_core::sender_auth::Challenge::new(
            NETWORK,
            [1; 32],
            0,
            &wallet.address(0).unwrap(),
            5,
            1_000,
            3_600,
        )
        .unwrap();
        let code = challenge.code();
        assert_eq!(code.len(), CHALLENGE_PREFIX.len() + CHALLENGE_SYMBOLS);
        assert_eq!(
            clean_code(&code, CHALLENGE_PREFIX).as_deref(),
            Some(code.as_str())
        );
        let response = genoryn_wallet_core::sender_auth::Response {
            challenge_id: [2; 32],
            signature: [3; 64],
        }
        .code();
        assert_eq!(response.len(), RESPONSE_PREFIX.len() + RESPONSE_SYMBOLS);
    }

    #[test]
    fn no_outcome_but_a_verified_one_is_shown_as_success() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        for outcome in ["not_on_chain", "reorganized", "unavailable"] {
            app.proofs.verified = Some(SecretJson(
                json!({"outcome": outcome, "txid": "ab", "chain_tip": 9}),
            ));
            app.proofs.sender_result = Some(Err("bad signature".into()));
            for light in [false, true] {
                set_style(&ctx, light);
                let output = ctx.run(Default::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.verify_page(ui));
                });
                for primitive in ctx.tessellate(output.shapes, output.pixels_per_point) {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        for vertex in mesh.vertices {
                            // Monochrome, or the error red: never a green success mark.
                            let c = vertex.color;
                            assert!(
                                c.r() == c.g() && c.g() == c.b() || c == ERROR || c.g() <= c.r(),
                                "{outcome}"
                            );
                        }
                    }
                }
            }
        }
    }
}
