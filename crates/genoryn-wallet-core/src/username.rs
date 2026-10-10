//! Signed off-chain receiver bundles. Local registry v1 is not a globally unique namespace.
use crate::keys::{Wallet, decode_address};
use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use genoryn_core::{codec, params::Network};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const MAX_LIFETIME: u64 = 365 * 24 * 60 * 60;
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ReceiverBundle {
    pub version: u16,
    pub network: Network,
    pub username: String,
    pub receiver: String,
    pub owner_key: [u8; 32],
    pub sequence: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub signature: Vec<u8>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Registry {
    pub entries: BTreeMap<String, ReceiverBundle>,
}
pub fn canonicalize(text: &str) -> Result<String> {
    let name = text.strip_prefix('@').unwrap_or(text).to_ascii_lowercase();
    ensure!(
        (3..=32).contains(&name.len())
            && name.as_bytes()[0].is_ascii_lowercase()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "username must use 3-32 ASCII letters/digits/underscores and start with a letter"
    );
    ensure!(
        ![
            "admin",
            "administrator",
            "genoryn",
            "support",
            "security",
            "treasury"
        ]
        .contains(&name.as_str()),
        "reserved username"
    );
    Ok(name)
}
impl ReceiverBundle {
    fn message(&self) -> Result<Vec<u8>> {
        let mut clone = self.clone();
        clone.signature.clear();
        let bytes = codec::encode(&clone)?;
        Ok(codec::hash(b"GENORYN/receiver-bundle/v1", &bytes).to_vec())
    }
    pub fn verify(&self, network: Network, now: u64) -> Result<()> {
        self.verify_for_renewal(network, now)?;
        ensure!(self.expires_at > now, "receiver bundle expired");
        Ok(())
    }
    /// Inspect an authenticated historical record for owner renewal only.
    /// Payment resolution must always use `verify`, which also enforces expiry.
    pub fn verify_for_renewal(&self, network: Network, now: u64) -> Result<()> {
        ensure!(
            self.version == 1 && self.network == network,
            "receiver bundle version/network mismatch"
        );
        ensure!(
            canonicalize(&self.username)? == self.username,
            "noncanonical username"
        );
        decode_address(network, &self.receiver)?;
        ensure!(
            self.issued_at <= now.saturating_add(120)
                && self.expires_at > self.issued_at
                && self.expires_at - self.issued_at <= MAX_LIFETIME,
            "expired or invalid receiver bundle time"
        );
        let signature = Signature::from_slice(&self.signature)?;
        VerifyingKey::from_bytes(&self.owner_key)?.verify_strict(&self.message()?, &signature)?;
        Ok(())
    }
    pub fn fingerprint(&self) -> String {
        hex::encode(codec::hash(b"GENORYN/username-owner/v1", &self.owner_key))
    }
}
impl Wallet {
    pub fn receiver_bundle(
        &self,
        username: &str,
        sequence: u64,
        now: u64,
    ) -> Result<ReceiverBundle> {
        let key = self.identity_key()?;
        let mut bundle = ReceiverBundle {
            version: 1,
            network: self.network,
            username: canonicalize(username)?,
            receiver: self.receiving_identity()?,
            owner_key: key.verifying_key().to_bytes(),
            sequence,
            issued_at: now,
            expires_at: now
                .checked_add(MAX_LIFETIME)
                .context("username expiry overflow")?,
            signature: vec![],
        };
        bundle.signature = key.sign(&bundle.message()?).to_bytes().to_vec();
        Ok(bundle)
    }
}
impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        ensure!(
            std::fs::metadata(path)?.len() <= 8_000_000,
            "registry file exceeds limit"
        );
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
    pub fn register(&mut self, bundle: ReceiverBundle, network: Network, now: u64) -> Result<()> {
        bundle.verify(network, now)?;
        if let Some(old) = self.entries.get(&bundle.username) {
            ensure!(
                old.owner_key == bundle.owner_key,
                "username belongs to another key; local v1 does not reassign expired names"
            );
            ensure!(
                bundle.sequence == old.sequence.checked_add(1).context("sequence overflow")?
                    && bundle.issued_at >= old.issued_at,
                "replayed receiver bundle"
            );
        } else {
            ensure!(
                bundle.sequence == 0,
                "first registration sequence must be zero"
            );
            ensure!(self.entries.len() < 10_000, "registry capacity reached");
        }
        self.entries.insert(bundle.username.clone(), bundle);
        Ok(())
    }
    pub fn resolve(&self, name: &str, network: Network, now: u64) -> Result<ReceiverBundle> {
        let bundle = self
            .entries
            .get(&canonicalize(name)?)
            .context("username is not registered with this resolver")?;
        bundle.verify(network, now)?;
        Ok(bundle.clone())
    }
    pub fn record_for_renewal(
        &self,
        name: &str,
        network: Network,
        now: u64,
    ) -> Result<ReceiverBundle> {
        let name = canonicalize(name)?;
        let bundle = self
            .entries
            .get(&name)
            .context("username is not registered")?;
        ensure!(bundle.username == name, "registry record name mismatch");
        bundle.verify_for_renewal(network, now)?;
        Ok(bundle.clone())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let temp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(self)?;
        let mut file = std::fs::File::create(&temp)?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_name_can_be_renewed_by_its_existing_owner() {
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let mut registry = Registry::default();
        registry
            .register(
                wallet.receiver_bundle("alice_renew", 0, 1000).unwrap(),
                Network::Devnet,
                1000,
            )
            .unwrap();
        let later = 1001 + MAX_LIFETIME;
        assert!(
            registry
                .resolve("alice_renew", Network::Devnet, later)
                .is_err()
        );
        let old = registry
            .record_for_renewal("alice_renew", Network::Devnet, later)
            .unwrap();
        registry
            .register(
                wallet
                    .receiver_bundle("alice_renew", old.sequence + 1, later)
                    .unwrap(),
                Network::Devnet,
                later,
            )
            .unwrap();
        assert_eq!(
            registry
                .resolve("alice_renew", Network::Devnet, later)
                .unwrap()
                .sequence,
            1
        );
    }
    #[test]
    fn ownership_replay_expiry_and_homographs() {
        for s in ["аlice", "a", "alice.com", "alice bob", "@admin"] {
            assert!(canonicalize(s).is_err());
        }
        assert_eq!(canonicalize("@Alice").unwrap(), "alice");
        let w = Wallet::create(Network::Devnet).unwrap();
        let b = w.receiver_bundle("alice", 0, 1000).unwrap();
        b.verify(Network::Devnet, 1001).unwrap();
        assert!(b.verify(Network::Testnet, 1001).is_err());
        let mut r = Registry::default();
        r.register(b.clone(), Network::Devnet, 1001).unwrap();
        assert!(r.register(b.clone(), Network::Devnet, 1001).is_err());
        assert!(
            r.resolve("alice", Network::Devnet, 1000 + MAX_LIFETIME)
                .is_err()
        );
        let evil = Wallet::create(Network::Devnet)
            .unwrap()
            .receiver_bundle("alice", 1, 1001)
            .unwrap();
        assert!(r.register(evil, Network::Devnet, 1001).is_err());
        let mut tamper = b;
        tamper.receiver = w.receiving_identity().unwrap() + "x";
        assert!(tamper.verify(Network::Devnet, 1001).is_err());
        r.register(
            w.receiver_bundle("alice", 1, 1002).unwrap(),
            Network::Devnet,
            1002,
        )
        .unwrap();
    }
}
