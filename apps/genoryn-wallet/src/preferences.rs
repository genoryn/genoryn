//! The only persisted wallet settings, in `DATA_DIR/wallet-preferences.json`.
//! Wallet file paths are remembered so the same wallets come back after a restart;
//! passwords, keys and balances never are.
use super::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE: &str = "wallet-preferences.json";
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub check_updates_at_launch: bool,
    /// Wallet files that were open, most recently active first.
    pub wallets: Vec<String>,
    /// Light appearance; dark is the default.
    pub light: bool,
    /// Network privacy mode for the node this wallet starts: `direct`, `hybrid`
    /// or `tor-only`. New installations start in `tor-only`; an installation
    /// from before Tor modes existed keeps `direct` (see `load`).
    #[serde(default = "earlier_mode")]
    pub network_mode: String,
    /// `built-in` (the Tor client inside the node; nothing to install) or
    /// `external` (a Tor daemon or Tor Browser the user runs). Empty in files
    /// written before the built-in client existed; see `sanitized`.
    #[serde(default)]
    pub tor_engine: String,
    /// External Tor's SOCKS proxy (IP:port).
    pub tor_socks: String,
    /// External Tor's optional control port (IP:port), for status and the onion service.
    pub tor_control: String,
    pub onion_service: bool,
    /// Light wallet: no node of its own; chain data and payments go through
    /// `light_sources` (remote nodes' public RPC, onion or HTTPS) over Tor.
    pub light_wallet: bool,
    pub light_sources: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            check_updates_at_launch: true,
            wallets: Vec::new(),
            light: false,
            network_mode: "tor-only".into(),
            tor_engine: network_privacy::Engine::BuiltIn.name().into(),
            tor_socks: network_privacy::DEFAULT_SOCKS.into(),
            tor_control: String::new(),
            onion_service: false,
            light_wallet: false,
            light_sources: Vec::new(),
        }
    }
}
impl Preferences {
    fn sanitized(mut self) -> Self {
        if network_privacy::Mode::parse(&self.network_mode).is_none() {
            self.network_mode = "direct".into();
        }
        if network_privacy::Engine::parse(&self.tor_engine).is_none() {
            // Someone already using a Tor mode ran their own Tor: keep it.
            self.tor_engine = if self.network_mode == "direct" {
                network_privacy::Engine::BuiltIn
            } else {
                network_privacy::Engine::External
            }
            .name()
            .into();
        }
        self.tor_socks = self.tor_socks.trim().to_owned();
        self.tor_control = self.tor_control.trim().to_owned();
        self.light_sources = self
            .light_sources
            .iter()
            .map(|source| source.trim().to_owned())
            .filter(|source| !source.is_empty())
            .take(network_privacy::MAX_LIGHT_SOURCES)
            .collect();
        let mut kept: Vec<String> = Vec::new();
        for path in self.wallets.drain(..) {
            let path = path.trim().to_owned();
            if !path.is_empty() && !kept.iter().any(|known| sessions::same_path(known, &path)) {
                kept.push(path);
            }
        }
        kept.truncate(sessions::MAX_OPEN_WALLETS);
        self.wallets = kept;
        self
    }
    /// Puts `path` first, as the wallet to preselect at the next launch.
    pub fn remember(&mut self, path: &str) {
        self.wallets
            .retain(|known| !sessions::same_path(known, path));
        self.wallets.insert(0, path.to_owned());
        self.wallets.truncate(sessions::MAX_OPEN_WALLETS);
    }
    pub fn forget(&mut self, path: &str) {
        self.wallets
            .retain(|known| !sessions::same_path(known, path));
    }
}

/// The mode of settings saved before the mode existed.
fn earlier_mode() -> String {
    "direct".into()
}
/// Whether `data_dir` already holds a node or wallet from an earlier
/// installation, which must not be switched to Tor silently.
fn existing_installation(data_dir: &Path) -> bool {
    let known = [FILE, "chain.sqlite", "admin.token", "wallet.gnr"];
    known.iter().any(|name| data_dir.join(name).exists())
        || std::fs::read_dir(data_dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "gnr"))
        })
}
pub(super) fn load(data_dir: &str) -> Preferences {
    let directory = PathBuf::from(data_dir);
    let path = directory.join(FILE);
    let readable =
        std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_BYTES);
    readable
        .then(|| std::fs::read(&path).ok())
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
        .unwrap_or_else(|| {
            if existing_installation(&directory) {
                Preferences {
                    network_mode: earlier_mode(),
                    ..Preferences::default()
                }
            } else {
                Preferences::default()
            }
        })
        .sanitized()
}
pub(super) fn save(data_dir: &str, preferences: &Preferences) -> Result<()> {
    let directory = PathBuf::from(data_dir);
    std::fs::create_dir_all(&directory)?;
    let temporary = directory.join(format!("{FILE}.tmp"));
    std::fs::write(&temporary, serde_json::to_vec_pretty(preferences)?)?;
    std::fs::rename(temporary, directory.join(FILE))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> String {
        std::env::temp_dir()
            .join(format!(
                "genoryn-preferences-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .display()
            .to_string()
    }

    #[test]
    fn preferences_round_trip_and_default_on() {
        let path = directory();
        assert_eq!(load(&path), Preferences::default());
        assert!(load(&path).check_updates_at_launch);
        let mut preferences = Preferences {
            check_updates_at_launch: false,
            light: true,
            network_mode: "tor-only".into(),
            ..Preferences::default()
        };
        preferences.remember("a/one.gnr");
        preferences.remember("a/two.gnr");
        save(&path, &preferences).unwrap();
        let loaded = load(&path);
        assert!(!loaded.check_updates_at_launch);
        assert!(loaded.light, "the appearance is remembered");
        assert_eq!(
            loaded.network_mode, "tor-only",
            "the privacy mode is remembered"
        );
        assert_eq!(loaded.wallets, ["a/two.gnr", "a/one.gnr"]);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn remembered_wallets_are_unique_bounded_and_ordered() {
        let mut preferences = Preferences::default();
        for n in 0..12 {
            preferences.remember(&format!("w{n}.gnr"));
        }
        assert_eq!(preferences.wallets.len(), sessions::MAX_OPEN_WALLETS);
        assert_eq!(preferences.wallets[0], "w11.gnr");
        preferences.remember("w5.gnr");
        assert_eq!(preferences.wallets[0], "w5.gnr");
        assert_eq!(
            preferences
                .wallets
                .iter()
                .filter(|w| *w == "w5.gnr")
                .count(),
            1
        );
        preferences.forget("w5.gnr");
        assert!(!preferences.wallets.contains(&"w5.gnr".to_owned()));
        let messy = Preferences {
            wallets: vec![" x.gnr ".into(), "x.gnr".into(), String::new()],
            network_mode: "onion-please".into(),
            ..Preferences::default()
        }
        .sanitized();
        assert_eq!(messy.wallets, ["x.gnr"]);
        assert_eq!(
            messy.network_mode, "direct",
            "an unknown mode never turns into a Tor mode or an error"
        );
    }

    #[test]
    fn earlier_tor_users_keep_their_own_tor_and_everyone_else_gets_the_built_in_one() {
        let path = directory();
        std::fs::create_dir_all(&path).unwrap();
        let file = PathBuf::from(&path).join(FILE);
        std::fs::write(
            &file,
            br#"{"network_mode":"tor-only","tor_socks":"127.0.0.1:9150"}"#,
        )
        .unwrap();
        let earlier = load(&path);
        assert_eq!(earlier.network_mode, "tor-only", "never changed silently");
        assert_eq!(earlier.tor_engine, "external");
        assert_eq!(earlier.tor_socks, "127.0.0.1:9150");
        std::fs::write(&file, br#"{"network_mode":"direct"}"#).unwrap();
        assert_eq!(load(&path).tor_engine, "built-in");
        assert_eq!(Preferences::default().tor_engine, "built-in");
        std::fs::write(&file, br#"{"network_mode":"hybrid","tor_engine":"bogus"}"#).unwrap();
        assert_eq!(load(&path).tor_engine, "external");
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn new_installations_start_in_tor_only_and_earlier_ones_keep_their_mode() {
        let fresh = directory();
        let new = load(&fresh);
        assert_eq!(new.network_mode, "tor-only");
        assert_eq!(new.tor_engine, "built-in", "nothing to install");

        let earlier = directory();
        std::fs::create_dir_all(&earlier).unwrap();
        std::fs::write(PathBuf::from(&earlier).join("chain.sqlite"), b"").unwrap();
        assert_eq!(
            load(&earlier).network_mode,
            "direct",
            "an existing node is never switched to Tor silently"
        );
        std::fs::remove_file(PathBuf::from(&earlier).join("chain.sqlite")).unwrap();
        std::fs::write(PathBuf::from(&earlier).join("savings.gnr"), b"").unwrap();
        assert_eq!(load(&earlier).network_mode, "direct");
        std::fs::write(
            PathBuf::from(&earlier).join(FILE),
            br#"{"check_updates_at_launch": true}"#,
        )
        .unwrap();
        assert_eq!(
            load(&earlier).network_mode,
            "direct",
            "settings saved before the mode existed"
        );
        std::fs::remove_dir_all(earlier).unwrap();
    }

    #[test]
    fn unreadable_or_foreign_files_fall_back_to_defaults() {
        let path = directory();
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(PathBuf::from(&path).join(FILE), b"not json").unwrap();
        let earlier = Preferences {
            network_mode: "direct".into(),
            ..Preferences::default()
        };
        assert_eq!(
            load(&path),
            earlier,
            "a damaged file is an earlier installation"
        );
        std::fs::write(
            PathBuf::from(&path).join(FILE),
            br#"{"check_updates_at_launch": true}"#,
        )
        .unwrap();
        assert_eq!(load(&path), earlier);
        std::fs::remove_dir_all(path).unwrap();
    }
}
