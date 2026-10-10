//! Peer labels for ordinary logs. In Tor modes, and for every onion address,
//! logs show a per-process salted tag instead of the address, so a shared log
//! does not reveal peers, onion identities or the operator's network position.
use super::addr::PeerAddr;
use rand08::RngCore;
use sha2::{Digest, Sha256};
use std::{fmt, sync::OnceLock};

fn salt() -> &'static [u8; 32] {
    static SALT: OnceLock<[u8; 32]> = OnceLock::new();
    SALT.get_or_init(|| {
        let mut salt = [0u8; 32];
        rand08::rngs::OsRng.fill_bytes(&mut salt);
        salt
    })
}

pub struct LogPeer<'a> {
    address: &'a PeerAddr,
    redact: bool,
}
pub fn peer(address: &PeerAddr, redact: bool) -> LogPeer<'_> {
    LogPeer { address, redact }
}
impl fmt::Display for LogPeer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.redact && !self.address.is_onion() {
            return write!(f, "{}", self.address);
        }
        let mut digest = Sha256::new();
        digest.update(salt());
        digest.update(self.address.to_string().as_bytes());
        write!(
            f,
            "{}#{}",
            self.address.class().name(),
            hex::encode(&digest.finalize()[..4])
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::addr::tests::test_onion;

    #[test]
    fn redacted_labels_hide_addresses_but_stay_stable_within_a_run() {
        let ip = PeerAddr::Ip("203.0.113.77:29610".parse().unwrap());
        let onion = PeerAddr::onion(test_onion(8), 29610);
        assert_eq!(peer(&ip, false).to_string(), "203.0.113.77:29610");
        let hidden = peer(&ip, true).to_string();
        assert!(hidden.starts_with("ipv4#") && !hidden.contains("203.0.113"));
        assert_eq!(hidden, peer(&ip, true).to_string());
        let onion_label = peer(&onion, false).to_string();
        assert!(onion_label.starts_with("onion#"));
        assert!(!onion_label.contains(&test_onion(8).hostname()[..16]));
    }
}
