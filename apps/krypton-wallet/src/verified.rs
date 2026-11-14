//! The Krypton coins the project verified: a list signed with the release key under its
//! own context (`docs/verified-assets.json`). Verification only says who issues a coin;
//! the wallet trusts no list that is not signed, and shows logos only of verified coins.
//!
//! The list compiled into this build is used until a newer signed one is fetched from
//! genoryn.io (checked every few hours).

use krypton_core::Network;
use krypton_core::asset::AssetId;
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CTX: &[u8] = b"genoryn-krypton/verified-coins/v1";
const RELEASE_KEY_HEX: &str = include_str!("../../../docs/release-key.pub");
const BUNDLED: &str = include_str!("../../../docs/verified-assets.json");
const BUNDLED_SIG: &str = include_str!("../../../docs/verified-assets.json.sig");
const URL: &str = "https://genoryn.io/verified-assets.json";

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct Entry {
    pub network: String,
    pub id: String,
    pub ticker: String,
    pub name: String,
    /// Who stands behind the coin, as the project checked it.
    #[serde(default)]
    pub issuer: String,
    /// Logo published by the project (shown only for verified coins).
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize)]
struct List {
    version: u32,
    updated: String,
    coins: Vec<Entry>,
}

struct Cache {
    fetched: Option<Instant>,
    updated: String,
    coins: Vec<Entry>,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        let (updated, coins) = check(BUNDLED.as_bytes(), BUNDLED_SIG).unwrap_or_default();
        Mutex::new(Cache {
            fetched: None,
            updated,
            coins,
        })
    })
}

/// The list if the release key signed it.
fn check(body: &[u8], sig_hex: &str) -> Option<(String, Vec<Entry>)> {
    let key = hex::decode(RELEASE_KEY_HEX.trim()).ok()?;
    let sig = hex::decode(sig_hex.trim()).ok()?;
    if !krypton_crypto::sign::hybrid_verify(&key, body, &sig, CTX) {
        return None;
    }
    let l: List = serde_json::from_slice(body).ok()?;
    (l.version == 1).then_some((l.updated, l.coins))
}

/// Fetches a newer signed list (at most every few hours; a bad or older one is ignored).
pub fn refresh() {
    if cache()
        .lock()
        .fetched
        .is_some_and(|t| t.elapsed() < Duration::from_secs(4 * 3600))
    {
        return;
    }
    cache().lock().fetched = Some(Instant::now());
    let Ok(c) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("genoryn-krypton-wallet")
        .build()
    else {
        return;
    };
    let get = |u: String| -> Option<Vec<u8>> {
        let r = c.get(u).send().ok()?.error_for_status().ok()?;
        let b = r.bytes().ok()?;
        (b.len() < 1 << 20).then(|| b.to_vec())
    };
    let (Some(body), Some(sig)) = (get(URL.into()), get(format!("{URL}.sig"))) else {
        return;
    };
    if let Some((updated, coins)) = check(&body, &String::from_utf8_lossy(&sig)) {
        let mut g = cache().lock();
        if updated >= g.updated {
            g.updated = updated;
            g.coins = coins;
        }
    }
}

/// Verified coins of `network`, by id.
pub fn list(network: Network) -> HashMap<AssetId, Entry> {
    cache()
        .lock()
        .coins
        .iter()
        .filter(|e| e.network == network.as_str())
        .filter_map(|e| Some((AssetId::from_hex(&e.id)?, e.clone())))
        .collect()
}

static NETWORK: OnceLock<Network> = OnceLock::new();

pub fn set_network(n: Network) {
    let _ = NETWORK.set(n);
}

pub fn network() -> Network {
    NETWORK.get().copied().unwrap_or(Network::Mainnet)
}

pub fn is_verified(id: &AssetId) -> bool {
    list(network()).contains_key(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_or_altered_lists_are_ignored() {
        assert!(check(b"{\"version\":1,\"updated\":\"x\",\"coins\":[]}", "00").is_none());
        // The bundled list is signed by the release key, and one changed byte voids it.
        let (_, coins) = check(BUNDLED.as_bytes(), BUNDLED_SIG).expect("bundled list signed");
        assert!(
            coins
                .iter()
                .any(|c| c.ticker == "USDK" && c.network == "mainnet")
        );
        let mut altered = BUNDLED.as_bytes().to_vec();
        altered.push(b' ');
        assert!(check(&altered, BUNDLED_SIG).is_none());
    }
}
