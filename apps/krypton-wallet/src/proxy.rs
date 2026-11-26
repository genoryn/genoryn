//! Sending the wallet's own connections (the nodes it reads, the offer board, the Bitcoin
//! API, the BTC price and updates) through a SOCKS5 proxy such as Tor, so the servers see
//! the proxy's address instead of this computer's.
//!
//! Every HTTP client in the wallet takes its proxy from the environment when it is built,
//! so the setting is applied once at start, before any thread or client exists; a change
//! takes effect at the next start. Only `socks5h` is accepted: names are then resolved by
//! the proxy, so no DNS lookup leaves this computer outside it. Connections to this
//! computer (a node or Bitcoin Core here) never go through the proxy.

use std::sync::atomic::{AtomicBool, Ordering};

const VARS: [&str; 6] = [
    "ALL_PROXY",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "all_proxy",
    "https_proxy",
    "http_proxy",
];
const LOCAL: &str = "localhost,127.0.0.1,::1";

static ON: AtomicBool = AtomicBool::new(false);

/// Checks a proxy setting: empty (no proxy) or `socks5h://host:port`.
pub fn check(p: &str) -> Result<(), &'static str> {
    const USE: &str = "use socks5h://host:port (Tor: socks5h://127.0.0.1:9050, Tor Browser: socks5h://127.0.0.1:9150), or leave it empty";
    if p.is_empty() {
        return Ok(());
    }
    if p.len() > 200 || p.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(USE);
    }
    let Ok(u) = reqwest::Url::parse(p) else {
        return Err(USE);
    };
    let ok = u.scheme() == "socks5h"
        && u.host_str().is_some_and(|h| !h.is_empty())
        && u.port().is_some()
        && matches!(u.path(), "" | "/")
        && u.query().is_none()
        && u.fragment().is_none();
    if ok { Ok(()) } else { Err(USE) }
}

/// Puts the proxy in place for this process. Must run before any other thread starts:
/// the environment is only changed while nothing else can read it.
pub fn apply(p: &str) {
    if check(p).is_err() || p.is_empty() {
        return;
    }
    for v in VARS {
        // SAFETY: called from `main` before the server, window or any other thread exists.
        unsafe { std::env::set_var(v, p) };
    }
    for v in ["NO_PROXY", "no_proxy"] {
        unsafe { std::env::set_var(v, LOCAL) };
    }
    ON.store(true, Ordering::Relaxed);
}

/// Whether this process sends its connections through the proxy.
pub fn active() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The node and the miner keep their own connections: they are not given the proxy
/// (the node's peer connections are not HTTP, and its own settings decide them).
pub fn strip(cmd: &mut std::process::Command) {
    if active() {
        for v in VARS.iter().chain(&["NO_PROXY", "no_proxy"]) {
            cmd.env_remove(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn only_socks5h_with_host_and_port() {
        for ok in [
            "",
            "socks5h://127.0.0.1:9050",
            "socks5h://127.0.0.1:9150",
            "socks5h://tor.lan:9050",
            "socks5h://user:pass@127.0.0.1:9050",
            "socks5h://[::1]:9050",
        ] {
            assert!(check(ok).is_ok(), "{ok}");
        }
        for bad in [
            "socks5://127.0.0.1:9050",
            "http://127.0.0.1:8080",
            "https://proxy.example:443",
            "socks5h://127.0.0.1",
            "socks5h://:9050",
            "socks5h://127.0.0.1:9050/x",
            "socks5h://127.0.0.1:9050?a=1",
            "socks5h://127.0.0.1:9050 ",
            "127.0.0.1:9050",
            "tor",
        ] {
            assert!(check(bad).is_err(), "{bad}");
        }
        assert!(check(&format!("socks5h://{}:9050", "a".repeat(200))).is_err());
    }
}
