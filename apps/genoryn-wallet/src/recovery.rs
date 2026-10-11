//! Wallet security: the 24-word recovery phrase, shown only after the wallet
//! password is typed again, and the wallet file re-encrypted under a new
//! password with the current key-derivation cost.
//!
//! The wallet never keeps the phrase. The CLI decrypts it from the file for one
//! request that carries the password typed in this window (never the one the
//! session holds). The words are drawn as non-selectable text (nothing to copy),
//! masked until asked for, wiped after a minute, when the window loses focus or
//! is minimized, on lock and on close, and while they are on screen the window
//! is kept out of screenshots, recordings and screen sharing where the system
//! allows it. Wrong passwords make the next attempt wait longer.
use super::*;
use genoryn_screen_guard::Protection;
use genoryn_wallet_core::keys::{self, StorageInfo};
use std::hash::{BuildHasher, Hasher};

/// How long revealed words stay readable.
const REVEAL_SECONDS: u64 = 60;
/// How long the backup check may take before the phrase is wiped.
const QUIZ_SECONDS: u64 = 180;
/// Wrong passwords allowed before each attempt has to wait.
const FREE_ATTEMPTS: u32 = 3;
const LONGEST_WAIT_SECONDS: u64 = 600;
const QUIZ_WORDS: usize = 3;
/// New and changed passwords: length and estimated strength.
pub(super) const MIN_PASSWORD_BITS: f64 = 50.0;

#[derive(Default)]
pub(super) struct RecoveryUi {
    pub open: bool,
    password: Zeroizing<String>,
    phrase: Zeroizing<String>,
    shown_at: Option<Instant>,
    visible: bool,
    quiz: Option<Quiz>,
    failures: u32,
    wait_until: Option<Instant>,
    error: Option<String>,
    pub change_open: bool,
    current: Zeroizing<String>,
    new_password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    /// The new password while its change is running; it becomes the session's.
    pending: Option<Zeroizing<String>>,
    change_error: Option<String>,
    change_done: Option<String>,
    file: Option<(String, Option<StorageInfo>)>,
}

struct Quiz {
    positions: [usize; QUIZ_WORDS],
    answers: [Zeroizing<String>; QUIZ_WORDS],
    result: Option<bool>,
}

impl RecoveryUi {
    /// Whether recovery words are in this window right now.
    pub fn words_on_screen(&self) -> bool {
        self.open && !self.phrase.is_empty()
    }
    /// Wipes the phrase and every password typed here.
    pub fn clear(&mut self) {
        self.open = false;
        self.change_open = false;
        self.forget_phrase();
        self.password.zeroize();
        self.current.zeroize();
        self.new_password.zeroize();
        self.confirm.zeroize();
        if let Some(mut pending) = self.pending.take() {
            pending.zeroize();
        }
        self.error = None;
        self.change_error = None;
        self.change_done = None;
    }
    fn forget_phrase(&mut self) {
        self.phrase.zeroize();
        self.shown_at = None;
        self.visible = false;
        if let Some(mut quiz) = self.quiz.take() {
            quiz.answers.iter_mut().for_each(|answer| answer.zeroize());
        }
    }
    fn wait_left(&self) -> Option<Duration> {
        self.wait_until
            .and_then(|until| until.checked_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }
    fn wrong_password(&mut self) {
        self.failures += 1;
        if self.failures >= FREE_ATTEMPTS {
            let seconds =
                (30u64 << (self.failures - FREE_ATTEMPTS).min(6)).min(LONGEST_WAIT_SECONDS);
            self.wait_until = Some(Instant::now() + Duration::from_secs(seconds));
        }
    }
    fn file_info(&mut self, path: &str) -> Option<StorageInfo> {
        if self.file.as_ref().is_none_or(|(known, _)| known != path) {
            self.file = Some((
                path.to_owned(),
                keys::storage_info(std::path::Path::new(path)).ok(),
            ));
        }
        self.file.as_ref().and_then(|(_, info)| *info)
    }
}

/// Estimated strength of a password in bits: the character classes it uses,
/// with repeats and runs (aaa, abc, 123) and very common words counted as
/// almost nothing. An estimate for guidance, not a guarantee.
pub(super) fn password_bits(password: &str) -> f64 {
    const COMMON: [&str; 22] = [
        "password", "passwort", "parol", "qwerty", "azerty", "123456", "654321", "111111",
        "000000", "letmein", "welcome", "admin", "iloveyou", "monkey", "dragon", "abc123",
        "genoryn", "wallet", "bitcoin", "crypto", "secret", "master",
    ];
    let mut text = password.to_lowercase();
    let mut bits = 0.0;
    for word in COMMON {
        while let Some(at) = text.find(word) {
            text.replace_range(at..at + word.len(), "\u{1}");
            bits += 6.0;
        }
    }
    let chars: Vec<char> = password.chars().collect();
    let mut pool = 0u32;
    if chars.iter().any(char::is_ascii_lowercase) {
        pool += 26;
    }
    if chars.iter().any(char::is_ascii_uppercase) {
        pool += 26;
    }
    if chars.iter().any(char::is_ascii_digit) {
        pool += 10;
    }
    if chars.iter().any(|c| c.is_ascii_punctuation() || *c == ' ') {
        pool += 33;
    }
    if chars.iter().any(|c| !c.is_ascii()) {
        pool += 100;
    }
    let each = f64::from(pool.max(1)).log2();
    let rest: Vec<char> = text.chars().filter(|c| *c != '\u{1}').collect();
    if !rest.is_empty() {
        bits += each;
        for pair in rest.windows(2) {
            let step = (i64::from(u32::from(pair[1])) - i64::from(u32::from(pair[0]))).abs();
            bits += if step <= 1 { 1.0 } else { each };
        }
    }
    bits
}

pub(super) fn strength_label(bits: f64) -> &'static str {
    match bits {
        b if b < MIN_PASSWORD_BITS => "Too easy to guess",
        b if b < 65.0 => "Fair",
        b if b < 80.0 => "Strong",
        _ => "Very strong",
    }
}

/// Why a new password cannot be used yet, `None` when it can.
pub(super) fn new_password_problem(password: &str) -> Option<String> {
    let length = password.chars().count();
    if length < MIN_NEW_PASSWORD_CHARS {
        return Some(format!(
            "Password is too short: {length} of at least {MIN_NEW_PASSWORD_CHARS} characters."
        ));
    }
    let bits = password_bits(password);
    (bits < MIN_PASSWORD_BITS).then(|| {
        format!("This password is easy to guess (about {bits:.0} bits). Use several unrelated words, or add digits and symbols.")
    })
}

pub(super) fn strength_meter(ui: &mut egui::Ui, password: &str) {
    if password.is_empty() {
        return;
    }
    let bits = password_bits(password);
    let fraction = (bits / 100.0).clamp(0.05, 1.0) as f32;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 6.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.inactive.bg_fill);
        let mut filled = rect;
        filled.set_width(rect.width() * fraction);
        let ink = if bits < MIN_PASSWORD_BITS {
            ERROR
        } else {
            ui.visuals().strong_text_color()
        };
        ui.painter().rect_filled(filled, 3.0, ink);
        ui.label(
            RichText::new(format!("{} · about {bits:.0} bits", strength_label(bits)))
                .small()
                .color(GRAY),
        );
    });
}

/// Three distinct word positions, different each time.
fn quiz_positions(words: usize) -> [usize; QUIZ_WORDS] {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    let mut seed = hasher.finish();
    let mut picked = [usize::MAX; QUIZ_WORDS];
    let mut count = 0;
    while count < QUIZ_WORDS {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let position = (seed >> 33) as usize % words.max(QUIZ_WORDS);
        if !picked[..count].contains(&position) {
            picked[count] = position;
            count += 1;
        }
    }
    picked.sort_unstable();
    picked
}

/// The words as a numbered grid that cannot be selected or copied.
pub(super) fn words_grid(ui: &mut egui::Ui, phrase: &str, visible: bool) {
    egui::Grid::new("recovery_words")
        .num_columns(4)
        .spacing([26.0, 12.0])
        .show(ui, |ui| {
            for (index, word) in phrase.split_whitespace().enumerate() {
                let shown = if visible { word } else { "••••••" };
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{:>2}  {shown}", index + 1))
                            .monospace()
                            .size(17.0),
                    )
                    .selectable(false),
                );
                if index % 4 == 3 {
                    ui.end_row();
                }
            }
        });
}

/// What the screen-capture protection achieved, in plain words.
pub(super) fn capture_note(ui: &mut egui::Ui, protection: Option<Protection>) {
    let (ok, text) = match protection {
        Some(Protection::Excluded) => (
            true,
            "Hidden from screenshots, screen recording and screen sharing while shown.",
        ),
        Some(Protection::Blacked) => (
            true,
            "Screenshots and screen sharing show this window as a black area while the words are shown.",
        ),
        _ => (
            false,
            "This system cannot hide the window from screen capture. Make sure no screen recording, screen sharing or remote-desktop session is running.",
        ),
    };
    payment_proof::verdict(ui, ok, RichText::new(text).small());
}

impl WalletApp {
    /// Settings → Wallet security.
    pub(super) fn wallet_security_card(&mut self, ui: &mut egui::Ui) {
        if !self.unlocked {
            return;
        }
        let path = self.settings.wallet_path.clone();
        let info = self.recovery.file_info(&path);
        card(ui, |ui| {
            small_title(ui, "Wallet security");
            ui.label("Your recovery phrase and wallet file are protected by your password. Without it, the file cannot be opened and the words are never shown.");
            ui.add_space(8.0);
            match info {
                Some(info) => {
                    diagnostic(
                        ui,
                        "Wallet file",
                        format!(
                            "version {}, Argon2id {} MiB × {} passes, XChaCha20-Poly1305",
                            info.version,
                            info.kdf.memory_kib / 1024,
                            info.kdf.passes
                        ),
                    );
                    if !info.is_current() {
                        ui.label(RichText::new("This file uses the lighter key derivation of earlier versions. Strengthen it below; you may keep the same password.").color(ERROR));
                    }
                }
                None => diagnostic(ui, "Wallet file", "could not be read"),
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                let idle = self.busy.is_none();
                if ui
                    .add_enabled(idle, primary(ui, "Show recovery phrase"))
                    .clicked()
                {
                    self.recovery.clear();
                    self.recovery.open = true;
                }
                let label = if info.is_some_and(|info| !info.is_current()) {
                    "Strengthen file / change password"
                } else {
                    "Change password"
                };
                if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                    self.recovery.clear();
                    self.recovery.change_open = true;
                }
            });
            if let Some(done) = &self.recovery.change_done {
                ui.add_space(6.0);
                payment_proof::verdict(ui, true, done.as_str());
            }
        });
        ui.add_space(18.0);
    }

    pub(super) fn recovery_windows(&mut self, ctx: &egui::Context) {
        if !self.unlocked {
            return;
        }
        // Words never outlive their minute, a hidden window or a lost focus.
        if let Some(shown) = self.recovery.shown_at {
            let limit = if self.recovery.quiz.is_some() {
                QUIZ_SECONDS
            } else {
                REVEAL_SECONDS
            };
            // Documentation captures run unfocused beside other work.
            let away = self.capture.is_none()
                && ctx.input(|i| {
                    i.viewport().focused == Some(false) || i.viewport().minimized == Some(true)
                });
            if shown.elapsed() >= Duration::from_secs(limit) || away {
                self.recovery.forget_phrase();
                self.recovery.error = Some(if away {
                    "The words were hidden because the window lost focus.".into()
                } else {
                    "The words were hidden after one minute.".into()
                });
            }
        }
        if self.recovery.open {
            self.phrase_window(ctx);
        }
        if self.recovery.change_open {
            self.change_password_window(ctx);
        }
    }

    fn phrase_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Recovery phrase")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if self.recovery.phrase.is_empty() {
                    self.phrase_password_step(ui);
                } else {
                    self.phrase_words_step(ui);
                }
            });
        if !open {
            self.recovery.clear();
        }
    }

    fn phrase_password_step(&mut self, ui: &mut egui::Ui) {
        ui.label("Anyone who sees these 24 words can take this wallet's funds, on any computer, without the file or the password.");
        ui.add_space(6.0);
        ui.label(RichText::new("Only continue alone, with no camera, screen sharing or remote session watching. GENORYN never asks for these words online.").color(GRAY));
        ui.add_space(12.0);
        ui.label("Type this wallet's password again");
        let wait = self.recovery.wait_left();
        let busy = self.busy.as_deref() == Some("reveal_phrase");
        let response = protocol::sensitive_edit(
            ui,
            wait.is_none() && !busy,
            field(&mut *self.recovery.password)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(error) = &self.recovery.error {
            ui.label(RichText::new(error.as_str()).color(ERROR));
        }
        if let Some(left) = wait {
            ui.label(
                RichText::new(format!(
                    "Too many wrong passwords. Try again in {} s.",
                    left.as_secs() + 1
                ))
                .color(ERROR),
            );
        }
        ui.add_space(10.0);
        let ready = !self.recovery.password.is_empty() && wait.is_none() && self.busy.is_none();
        if ui
            .add_enabled(
                ready,
                primary(
                    ui,
                    if busy {
                        "Checking…"
                    } else {
                        "Show recovery phrase"
                    },
                ),
            )
            .clicked()
            || (submitted && ready)
        {
            let typed = Zeroizing::new(std::mem::take(&mut *self.recovery.password));
            self.recovery.error = None;
            self.dispatch(
                "reveal_phrase",
                "reveal_phrase".into(),
                Some((self.settings.wallet_path.clone(), typed)),
                json!({}),
            );
        }
    }

    fn phrase_words_step(&mut self, ui: &mut egui::Ui) {
        let left = self
            .recovery
            .shown_at
            .map(|shown| REVEAL_SECONDS.saturating_sub(shown.elapsed().as_secs()))
            .unwrap_or_default();
        capture_note(ui, self.capture_guard);
        ui.add_space(10.0);
        if let Some(quiz) = &mut self.recovery.quiz {
            ui.label(RichText::new("Check your paper copy").strong());
            ui.label("Type these words from your paper, not from memory.");
            ui.add_space(8.0);
            let words: Vec<&str> = self.recovery.phrase.split_whitespace().collect();
            for (position, answer) in quiz.positions.iter().zip(quiz.answers.iter_mut()) {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [80.0, 20.0],
                        egui::Label::new(format!("Word {}", position + 1)).selectable(false),
                    );
                    protocol::sensitive_edit(
                        ui,
                        quiz.result.is_none(),
                        field(&mut **answer).password(true).desired_width(220.0),
                    );
                });
            }
            ui.add_space(8.0);
            match quiz.result {
                None => {
                    if ui
                        .add_enabled(
                            quiz.answers.iter().all(|a| !a.trim().is_empty()),
                            primary(ui, "Check"),
                        )
                        .clicked()
                    {
                        let right = quiz.positions.iter().zip(quiz.answers.iter()).all(
                            |(position, answer)| {
                                words
                                    .get(*position)
                                    .is_some_and(|word| answer.trim().eq_ignore_ascii_case(word))
                            },
                        );
                        quiz.result = Some(right);
                        if right {
                            self.preferences.backup_confirmed.insert(
                                sessions::path_key(&self.settings.wallet_path),
                                genoryn_core::chain::unix_time(),
                            );
                        }
                    }
                }
                Some(true) => payment_proof::verdict(
                    ui,
                    true,
                    "Your backup matches. Keep the paper offline and safe.",
                ),
                Some(false) => payment_proof::verdict(
                    ui,
                    false,
                    "These words do not match. Check your paper copy against the phrase again.",
                ),
            }
            if quiz.result == Some(true) {
                self.save_preferences();
                self.recovery.forget_phrase();
                self.recovery.change_done =
                    Some("Backup checked: three random words matched your paper copy.".into());
                self.recovery.open = false;
            } else if quiz.result == Some(false) && ui.button("Show the words again").clicked() {
                if let Some(mut quiz) = self.recovery.quiz.take() {
                    quiz.answers.iter_mut().for_each(|answer| answer.zeroize());
                }
                self.recovery.shown_at = Some(Instant::now());
            }
            return;
        }
        words_grid(ui, &self.recovery.phrase, self.recovery.visible);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .button(if self.recovery.visible {
                    "Hide words"
                } else {
                    "Show words"
                })
                .clicked()
            {
                self.recovery.visible = !self.recovery.visible;
            }
            if ui.button("Check my backup").clicked() {
                self.recovery.visible = false;
                self.recovery.quiz = Some(Quiz {
                    positions: quiz_positions(self.recovery.phrase.split_whitespace().count()),
                    answers: Default::default(),
                    result: None,
                });
                self.recovery.shown_at = Some(Instant::now());
            }
            if ui.button("Done").clicked() {
                self.recovery.clear();
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new(format!("Hidden again in {left} s. The words cannot be copied: write them on paper, in order. Never photograph them or type them into a computer, phone or website.")).small().color(GRAY));
    }

    fn change_password_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let path = self.settings.wallet_path.clone();
        let current_kdf = self.recovery.file_info(&path);
        egui::Window::new("Change wallet password")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("The wallet file is encrypted again with your new password and the current, stronger key derivation (Argon2id 256 MiB). Enter the same password twice to only strengthen the file.");
                ui.add_space(6.0);
                ui.label(RichText::new("The new file is written and checked before it replaces the old one; if anything fails, the old file stays as it is. Copies of the old file elsewhere (backups) keep the old password.").small().color(GRAY));
                if let Some(info) = current_kdf {
                    ui.label(RichText::new(format!("Now: version {}, Argon2id {} MiB × {} passes.", info.version, info.kdf.memory_kib / 1024, info.kdf.passes)).small().color(GRAY));
                }
                ui.add_space(10.0);
                let wait = self.recovery.wait_left();
                let busy = self.busy.as_deref() == Some("change_password");
                let enabled = wait.is_none() && !busy;
                ui.label("Current password");
                protocol::sensitive_edit(ui, enabled, field(&mut *self.recovery.current).password(true).desired_width(f32::INFINITY));
                ui.label(format!("New password (at least {MIN_NEW_PASSWORD_CHARS} characters)"));
                protocol::sensitive_edit(ui, enabled, field(&mut *self.recovery.new_password).password(true).desired_width(f32::INFINITY));
                strength_meter(ui, &self.recovery.new_password);
                ui.label("Repeat the new password");
                protocol::sensitive_edit(ui, enabled, field(&mut *self.recovery.confirm).password(true).desired_width(f32::INFINITY));
                let problem = if self.recovery.current.is_empty() {
                    Some("Enter the current password.".to_owned())
                } else if *self.recovery.new_password == *self.recovery.current {
                    None
                } else {
                    new_password_problem(&self.recovery.new_password)
                }
                .or_else(|| (*self.recovery.confirm != *self.recovery.new_password).then(|| "Repeat the same new password.".to_owned()));
                if let Some(error) = &self.recovery.change_error {
                    ui.label(RichText::new(error.as_str()).color(ERROR));
                }
                if let Some(left) = wait {
                    ui.label(RichText::new(format!("Too many wrong passwords. Try again in {} s.", left.as_secs() + 1)).color(ERROR));
                }
                if let Some(problem) = &problem {
                    ui.label(RichText::new(problem.as_str()).color(GRAY));
                }
                ui.add_space(10.0);
                let same = *self.recovery.new_password == *self.recovery.current;
                let ready = problem.is_none() && wait.is_none() && self.busy.is_none() && self.scan_busy.is_none();
                if ui.add_enabled(ready, primary(ui, if busy { "Re-encrypting…" } else if same { "Strengthen wallet file" } else { "Change password" })).clicked() {
                    let current = Zeroizing::new(std::mem::take(&mut *self.recovery.current));
                    let new_password = Zeroizing::new(std::mem::take(&mut *self.recovery.new_password));
                    self.recovery.confirm.zeroize();
                    self.recovery.change_error = None;
                    let extra = json!({"new_password": new_password.as_str()});
                    self.recovery.pending = Some(new_password);
                    self.dispatch("change_password", "change_password".into(), Some((path.clone(), current)), extra);
                }
            });
        if !open {
            self.recovery.clear();
        }
    }

    /// Documentation screenshots: the phrase window, then its words masked.
    /// Returns whether the window is ready to be captured.
    pub(super) fn capture_recovery(&mut self, masked: bool) -> bool {
        if !self.recovery.open {
            self.recovery.clear();
            self.recovery.open = true;
            return false;
        }
        if !masked {
            return true;
        }
        if self.recovery.phrase.is_empty() {
            if self.busy.is_none() {
                let credentials = (self.settings.wallet_path.clone(), self.password.clone());
                self.dispatch(
                    "reveal_phrase",
                    "reveal_phrase".into(),
                    Some(credentials),
                    json!({}),
                );
            }
            return false;
        }
        self.recovery.visible = false;
        true
    }

    /// The CLI's answer to `reveal_phrase`.
    pub(super) fn accept_phrase(&mut self, result: &Value) {
        let words = result["seed_phrase"].as_str().unwrap_or_default();
        if !self.recovery.open || words.split_whitespace().count() != RECOVERY_WORDS {
            return;
        }
        self.recovery.failures = 0;
        self.recovery.wait_until = None;
        *self.recovery.phrase = words.to_owned();
        self.recovery.shown_at = Some(Instant::now());
        self.recovery.visible = false;
    }

    /// The CLI's answer to `change_password`: the session keeps working with the new password.
    pub(super) fn accept_password_change(&mut self, result: &Value) {
        self.recovery.failures = 0;
        self.recovery.wait_until = None;
        if let Some(new_password) = self.recovery.pending.take() {
            self.password = new_password;
        }
        self.recovery.file = None;
        self.recovery.change_open = false;
        self.recovery.change_done = Some(format!(
            "Password changed. The wallet file now uses Argon2id {} MiB × {} passes.",
            result["kdf_memory_mib"], result["kdf_passes"]
        ));
        self.status =
            "Wallet password changed. Older copies of this file still open with the old password."
                .into();
    }

    pub(super) fn recovery_failed(&mut self, op: &str, error: &str) {
        let wrong = error.contains("wrong password");
        if wrong {
            self.recovery.wrong_password();
        }
        let message = if wrong { "Wrong password." } else { error };
        match op {
            "reveal_phrase" => self.recovery.error = Some(message.into()),
            _ => {
                if let Some(mut pending) = self.recovery.pending.take() {
                    pending.zeroize();
                }
                self.recovery.change_error = Some(message.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_estimate_rejects_patterns_and_common_words() {
        for weak in [
            "aaaaaaaaaaaa",
            "123456789012",
            "passwordpassword",
            "qwertyqwerty1",
            "abcdefghijkl",
            "genorynwallet",
        ] {
            assert!(
                password_bits(weak) < MIN_PASSWORD_BITS,
                "{weak}: {}",
                password_bits(weak)
            );
            assert!(new_password_problem(weak).is_some(), "{weak}");
        }
        for strong in [
            "correct-horse-battery",
            "Tbilisi river 7 lamps",
            "mzis-sxivi-qva-2026",
            "გამარჯობა-ქვა-91",
        ] {
            assert!(
                password_bits(strong) >= 65.0,
                "{strong}: {}",
                password_bits(strong)
            );
            assert!(new_password_problem(strong).is_none(), "{strong}");
        }
        assert!(
            new_password_problem("Sh0rt!x")
                .unwrap()
                .contains("too short")
        );
    }

    #[test]
    fn quiz_positions_are_distinct_and_in_range() {
        for _ in 0..200 {
            let positions = quiz_positions(24);
            assert!(positions.iter().all(|p| *p < 24));
            assert!(positions[0] < positions[1] && positions[1] < positions[2]);
        }
    }

    #[test]
    fn wrong_passwords_wait_longer_and_clear_wipes_everything() {
        let mut ui = RecoveryUi::default();
        for _ in 0..FREE_ATTEMPTS - 1 {
            ui.wrong_password();
        }
        assert!(ui.wait_left().is_none());
        ui.wrong_password();
        let first = ui.wait_left().unwrap();
        assert!(first <= Duration::from_secs(30) && first > Duration::from_secs(25));
        ui.wrong_password();
        assert!(ui.wait_left().unwrap() > Duration::from_secs(55));
        for _ in 0..20 {
            ui.wrong_password();
        }
        assert!(ui.wait_left().unwrap() <= Duration::from_secs(LONGEST_WAIT_SECONDS));
        ui.open = true;
        *ui.phrase = "word ".repeat(24);
        *ui.password = "typed".into();
        ui.pending = Some(Zeroizing::new("next".into()));
        assert!(ui.words_on_screen());
        ui.clear();
        assert!(
            !ui.words_on_screen()
                && ui.phrase.is_empty()
                && ui.password.is_empty()
                && ui.pending.is_none()
        );
    }
}
