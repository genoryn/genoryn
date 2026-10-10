//! On-chain usernames in the wallet: live availability, the wallet's own names and
//! registration. Lookups are public read requests to the wallet's own node and run
//! off the UI thread; registering burns the price in an ordinary private transfer.
use super::*;
use genoryn_core::names;
use std::collections::{BTreeMap, BTreeSet};

const LOOKUP_DELAY: Duration = Duration::from_millis(450);
const LOOKUP_FRESH: Duration = Duration::from_secs(10);
const MY_NAMES_REFRESH: Duration = Duration::from_secs(20);
/// The CLI's default fee for a name transaction, in gori.
const REGISTRATION_FEE: u64 = 10_000_000;

#[derive(Default)]
pub(super) struct NamesUi {
    pub input: String,
    pub edited: Option<Instant>,
    pub cache: BTreeMap<String, (Instant, Result<Value, String>)>,
    pub inflight: BTreeSet<String>,
    /// Public owner key of the active wallet, reported by its scan.
    pub owner: Option<String>,
    pub receiver: Option<String>,
    pub mine: Vec<String>,
    pub pending: Vec<String>,
    pub mine_checked: Option<Instant>,
    pub mine_inflight: bool,
    pub confirm: Option<(String, String)>,
}

fn fetch(rpc_url: &str, path: &str) -> Result<Value> {
    network_privacy::guard_rpc(rpc_url)?;
    let base = genoryn_connect::validate_endpoint(rpc_url, NETWORK.name())?;
    let url = base.join(path)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let response = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()?
                .get(url)
                .send()
                .await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                anyhow::bail!("this node does not support on-chain usernames yet; update GENORYN");
            }
            let status = response.status();
            let body: Value = response.json().await?;
            ensure!(
                status.is_success(),
                "{}",
                body["error"]
                    .as_str()
                    .unwrap_or("the node rejected the lookup")
            );
            Ok(body)
        })
}

impl WalletApp {
    pub(super) fn lookup_name(&mut self, name: &str) {
        let Ok(name) = names::canonical_name(name) else {
            return;
        };
        let fresh = self
            .names
            .cache
            .get(&name)
            .is_some_and(|(at, _)| at.elapsed() < LOOKUP_FRESH);
        if fresh || !self.names.inflight.insert(name.clone()) {
            return;
        }
        let (tx, rpc) = (self.tx.clone(), self.settings.rpc_url.clone());
        thread::spawn(move || {
            let result = fetch(&rpc, &format!("v1/names?name={name}")).map_err(|e| e.to_string());
            let _ = tx.send(Event::NameStatus { name, result });
        });
    }
    fn refresh_my_names(&mut self) {
        let Some(owner) = self.names.owner.clone() else {
            return;
        };
        if self.names.mine_inflight {
            return;
        }
        self.names.mine_inflight = true;
        self.names.mine_checked = Some(Instant::now());
        let (tx, rpc) = (self.tx.clone(), self.settings.rpc_url.clone());
        thread::spawn(move || {
            let result =
                fetch(&rpc, &format!("v1/names/owner?owner={owner}")).map_err(|e| e.to_string());
            let _ = tx.send(Event::MyNames { owner, result });
        });
    }
    /// Debounced lookups for what the user is typing, and a periodic refresh of the
    /// wallet's own names.
    pub(super) fn names_tick(&mut self) {
        if !self.unlocked {
            return;
        }
        if self
            .names
            .edited
            .is_some_and(|edited| edited.elapsed() >= LOOKUP_DELAY)
        {
            self.names.edited = None;
            let input = self.names.input.clone();
            self.lookup_name(&input);
        }
        if self.to.starts_with('@') {
            let to = self.to.clone();
            self.lookup_name(&to);
        }
        if self
            .names
            .mine_checked
            .is_none_or(|checked| checked.elapsed() >= MY_NAMES_REFRESH)
        {
            self.refresh_my_names();
        }
    }
    pub(super) fn names_event(&mut self, event: Event) {
        match event {
            Event::NameStatus { name, result } => {
                self.names.inflight.remove(&name);
                self.names.cache.insert(name, (Instant::now(), result));
            }
            Event::MyNames { owner, result } => {
                self.names.mine_inflight = false;
                if self.names.owner.as_deref() != Some(owner.as_str()) {
                    return;
                }
                if let Ok(value) = result {
                    self.names.mine = value["names"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .filter_map(|n| n["name"].as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    self.names.pending = value["pending"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .filter_map(|n| n.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                }
            }
            _ => {}
        }
    }
    /// Called with each scan result of the active wallet.
    pub(super) fn set_name_identity(&mut self, summary: &Value) {
        let owner = summary["name_owner"].as_str().map(str::to_owned);
        if owner != self.names.owner {
            self.names.mine.clear();
            self.names.pending.clear();
            self.names.mine_checked = None;
        }
        self.names.owner = owner;
        self.names.receiver = summary["name_receiver"].as_str().map(str::to_owned);
    }
    pub(super) fn forget_name_identity(&mut self) {
        let input = std::mem::take(&mut self.names.input);
        self.names = NamesUi {
            input,
            ..NamesUi::default()
        };
    }
    fn cached_status(&self, name: &str) -> Option<&Result<Value, String>> {
        let name = names::canonical_name(name).ok()?;
        self.names.cache.get(&name).map(|(_, result)| result)
    }
    /// One line under the recipient field: what `@name` resolves to on the chain.
    pub(super) fn recipient_name_hint(&self, ui: &mut egui::Ui) {
        if !self.to.starts_with('@') {
            return;
        }
        let text = match names::canonical_name(&self.to) {
            Err(error) => error.to_string(),
            Ok(name) => match self.cached_status(&name) {
                None => format!("Looking up @{name} on the chain…"),
                Some(Err(error)) => error.clone(),
                Some(Ok(status)) => match status["registered"]["receiver"].as_str() {
                    Some(receiver) => format!(
                        "@{name} → {receiver} · registered at block {}",
                        status["registered"]["registered_height"]
                    ),
                    None if status["pending"] == true => {
                        format!("@{name} is being registered; wait for the next block")
                    }
                    None => format!("@{name} is not registered on the chain"),
                },
            },
        };
        ui.label(RichText::new(text).small().color(GRAY));
    }
    /// The Receive page's username card.
    pub(super) fn names_section(&mut self, ui: &mut egui::Ui) {
        small_title(ui, "USERNAME");
        if self.names.mine.is_empty() && self.names.pending.is_empty() {
            ui.label("Get a username so people can pay @you instead of a long address.");
        } else {
            ui.horizontal_wrapped(|ui| {
                for name in &self.names.mine {
                    ui.label(RichText::new(format!("@{name}")).size(20.0).strong());
                }
                for name in &self.names.pending {
                    ui.label(
                        RichText::new(format!("@{name} · confirming"))
                            .size(16.0)
                            .color(GRAY),
                    );
                }
            });
            if let Some(receiver) = &self.names.receiver {
                ui.label(
                    RichText::new(format!("Payments to your username arrive at {receiver}"))
                        .small()
                        .color(GRAY),
                );
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    field(&mut self.names.input)
                        .hint_text("@yourname")
                        .char_limit(33)
                        .desired_width(320.0),
                )
                .changed()
            {
                self.names.edited = Some(Instant::now());
            }
            let ready = names::canonical_name(&self.names.input).ok().filter(|name| {
                matches!(self.cached_status(name), Some(Ok(status)) if status["available"] == true)
            });
            let short = ready
                .as_ref()
                .and_then(|name| self.registration_shortfall(name));
            if ui
                .add_enabled(
                    ready.is_some() && short.is_none() && self.busy.is_none(),
                    primary(ui, "Register username"),
                )
                .clicked()
                && let Some(name) = ready
            {
                let price = self
                    .cached_status(&name)
                    .and_then(|status| status.as_ref().ok())
                    .and_then(|status| status["price_atomic"].as_str())
                    .unwrap_or("0")
                    .to_owned();
                self.names.confirm = Some((name, price));
            }
        });
        let (text, strong) = if self.names.input.trim().is_empty() {
            (String::new(), false)
        } else {
            match names::canonical_name(&self.names.input) {
                Err(error) => (error.to_string(), true),
                Ok(name) => match self.cached_status(&name) {
                    None => ("Checking…".to_owned(), false),
                    Some(Err(error)) => (error.clone(), true),
                    Some(Ok(status)) if status["available"] == true => (
                        format!(
                            "@{name} is available · {} GNR, burned forever",
                            format_atomic(status["price_atomic"].as_str().unwrap_or("0"))
                        ),
                        false,
                    ),
                    Some(Ok(status)) if status["pending"] == true => {
                        (format!("@{name} is being registered by someone"), true)
                    }
                    Some(Ok(_)) => (format!("@{name} is taken"), true),
                },
            }
        };
        if !text.is_empty() {
            ui.label(RichText::new(text).color(if strong { ERROR } else { GRAY }));
        }
        let ready = names::canonical_name(&self.names.input)
            .ok()
            .filter(|name| matches!(self.cached_status(name), Some(Ok(status)) if status["available"] == true));
        if let Some(reason) = ready
            .and_then(|name| self.registration_shortfall(&name))
            .or_else(|| self.action_error_for("register_name").map(str::to_owned))
        {
            ui.label(RichText::new(reason).color(ERROR));
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Prices: 3–4 letters 1,000 GNR · 5–6 letters 100 GNR · 7 or more 10 GNR. The price is burned, so nobody receives it. A username is public and permanent; payments to it stay private.",
            )
            .small()
            .color(GRAY),
        );
    }
    /// Price plus the default network fee, against the matured balance.
    fn registration_shortfall(&self, name: &str) -> Option<String> {
        self.shortfall(names::registration_price(name).checked_add(REGISTRATION_FEE)?)
    }
    #[cfg(test)]
    pub(super) fn name_available(&self, name: &str) -> Option<bool> {
        match self.cached_status(name)? {
            Ok(status) => Some(status["available"] == true),
            Err(_) => None,
        }
    }
    pub(super) fn name_confirm_window(&mut self, ctx: &egui::Context) {
        let Some((name, price)) = self.names.confirm.clone() else {
            return;
        };
        let mut open = true;
        let mut register = false;
        egui::Window::new("Register username")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(460.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("@{name}")).size(26.0).strong());
                let short = self.registration_shortfall(&name);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Burned:");
                    gnr_amount(ui, Some(&price), 16.0, None);
                });
                ui.label("Plus the ordinary network fee.");
                ui.add_space(8.0);
                for line in [
                    "The name is yours forever and cannot be sold back; the price is destroyed.",
                    "Everyone can see that @name points to your username address. Your main address, balance and payments stay private.",
                    "If someone registers it in the same block first, your registration has no effect and the burned amount is lost.",
                ] {
                    ui.label(RichText::new(format!("• {line}")).small());
                }
                ui.add_space(12.0);
                if let Some(reason) = &short {
                    ui.label(RichText::new(reason.as_str()).color(ERROR));
                    ui.add_space(8.0);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.busy.is_none() && short.is_none(),
                            primary(ui, "Burn and register"),
                        )
                        .clicked()
                    {
                        register = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.names.confirm = None;
                    }
                });
            });
        if register {
            self.names.confirm = None;
            self.action_error = None;
            self.request("register_name", json!({ "name": name }));
        } else if !open {
            self.names.confirm = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_are_cached_by_canonical_name_and_owners_never_mix() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.names_event(Event::NameStatus {
            name: "maxsavings".into(),
            result: Ok(json!({"available": true, "price_atomic": "10000000000000"})),
        });
        assert_eq!(app.name_available("@MaxSavings"), Some(true));
        assert_eq!(app.name_available("other"), None);

        app.set_name_identity(&json!({"name_owner": "aa", "name_receiver": "gnr1x"}));
        app.names_event(Event::MyNames {
            owner: "aa".into(),
            result: Ok(json!({"names": [{"name": "maxsavings"}], "pending": ["maxi"]})),
        });
        assert_eq!(app.names.mine, ["maxsavings"]);
        assert_eq!(app.names.pending, ["maxi"]);
        app.names_event(Event::MyNames {
            owner: "bb".into(),
            result: Ok(json!({"names": [{"name": "intruder"}], "pending": []})),
        });
        assert_eq!(
            app.names.mine,
            ["maxsavings"],
            "a late reply for another owner is ignored"
        );
        app.set_name_identity(&json!({"name_owner": "bb"}));
        assert!(
            app.names.mine.is_empty(),
            "switching wallets clears the previous names"
        );
        app.lock();
        assert!(app.names.owner.is_none() && app.names.cache.is_empty());
    }

    #[test]
    fn a_miner_paying_another_wallet_is_reported() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        app.address = "gnr1open".into();
        app.miner_destination = Some(("gnr1mining".into(), "Mining".into()));
        assert_eq!(app.mining_elsewhere(), None, "no miner running");
        app.miner_retry_at = Some(Instant::now());
        assert_eq!(app.mining_elsewhere(), Some("Mining"));
        app.address = "gnr1mining".into();
        assert_eq!(app.mining_elsewhere(), None, "viewing the paid wallet");
    }

    #[test]
    fn registration_and_payment_need_matured_balance() {
        let ctx = egui::Context::default();
        let mut app = WalletApp::with_context(&ctx, Settings::default());
        assert_eq!(
            app.registration_shortfall("maxsavings"),
            None,
            "unknown balance does not block"
        );
        app.spendable = Some((5 * names::GNR).to_string());
        let reason = app.registration_shortfall("maxsavings").unwrap();
        assert!(
            reason.contains("10.00001 GNR needed") && reason.contains("5 GNR spendable"),
            "{reason}"
        );
        assert!(app.registration_shortfall("drogoz").is_some(), "100 GNR");
        app.spendable = Some((10 * names::GNR + REGISTRATION_FEE).to_string());
        assert_eq!(
            app.registration_shortfall("maxsavings"),
            None,
            "exactly enough"
        );
        assert!(
            app.shortfall(10 * names::GNR + REGISTRATION_FEE + 1)
                .is_some()
        );
    }
}
