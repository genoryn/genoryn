use anyhow::{Context, Result, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use bip39::Mnemonic;
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305,
    aead::{Aead, Payload},
};
use genoryn_core::{codec, params::Network};
use orchard::{
    Address,
    keys::{FullViewingKey, Scope, SpendingKey},
};
use rand08::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use zeroize::Zeroizing;

pub struct Wallet {
    pub network: Network,
    entropy: Zeroizing<Vec<u8>>,
    pub account: u32,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u16,
    network: Network,
    salt: [u8; 16],
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

impl Wallet {
    pub fn create(network: Network) -> Result<Self> {
        network.ensure_runnable()?;
        let mut entropy = Zeroizing::new(vec![0u8; 32]);
        rand08::rngs::OsRng.fill_bytes(&mut entropy);
        Ok(Self {
            network,
            entropy,
            account: 0,
        })
    }
    pub fn restore(network: Network, phrase: &str) -> Result<Self> {
        network.ensure_runnable()?;
        let mnemonic = Mnemonic::parse(phrase).context("invalid recovery phrase or checksum")?;
        ensure!(
            mnemonic.word_count() == 24,
            "GENORYN recovery requires a 24-word BIP39 phrase"
        );
        Ok(Self {
            network,
            entropy: Zeroizing::new(mnemonic.to_entropy()),
            account: 0,
        })
    }
    pub fn phrase(&self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(
            Mnemonic::from_entropy(&self.entropy)?.to_string(),
        ))
    }
    /// Derive a separate ZIP32 account from the same recovery seed.
    /// Account number is recovery metadata; this does not modify an existing file.
    pub fn for_account(&self, account: u32) -> Result<Self> {
        ensure!(account < (1u32 << 31), "account index exceeds ZIP32 range");
        Ok(Self {
            network: self.network,
            entropy: Zeroizing::new(self.entropy.to_vec()),
            account,
        })
    }
    fn seed(&self) -> Result<Zeroizing<[u8; 64]>> {
        Ok(Zeroizing::new(
            Mnemonic::from_entropy(&self.entropy)?.to_seed(""),
        ))
    }
    pub fn spending_key(&self) -> Result<SpendingKey> {
        let seed = self.seed()?;
        // Experimental GENORYN coin-type domain; not a claimed SLIP-0044 assignment.
        Ok(SpendingKey::from_zip32_seed(
            &*seed,
            0x474e00 + self.network.id() as u32,
            self.account
                .try_into()
                .map_err(|_| anyhow::anyhow!("account index exceeds ZIP32 range"))?,
        )?)
    }
    pub fn viewing_key(&self) -> Result<FullViewingKey> {
        Ok(FullViewingKey::from(&self.spending_key()?))
    }
    pub fn address(&self, index: u32) -> Result<Address> {
        Ok(self.viewing_key()?.address_at(index, Scope::External))
    }
    pub fn receiving_identity(&self) -> Result<String> {
        encode_address(self.network, self.address(0)?)
    }
    pub fn identity_key(&self) -> Result<ed25519_dalek::SigningKey> {
        let seed = self.seed()?;
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(b"GENORYN/username/v1"), &*seed);
        let mut key = Zeroizing::new([0u8; 32]);
        hk.expand(&[self.network.id()], &mut *key)
            .map_err(|_| anyhow::anyhow!("identity derivation failed"))?;
        Ok(ed25519_dalek::SigningKey::from_bytes(&key))
    }
    /// Encrypts the local scan cache; distinct from every spending/viewing key.
    pub fn scan_cache_key(&self) -> Result<Zeroizing<[u8; 32]>> {
        let seed = self.seed()?;
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(b"GENORYN/scan-cache/v1"), &*seed);
        let mut key = Zeroizing::new([0u8; 32]);
        let mut info = vec![self.network.id()];
        info.extend_from_slice(&self.account.to_le_bytes());
        hk.expand(&info, &mut *key)
            .map_err(|_| anyhow::anyhow!("scan cache key derivation failed"))?;
        Ok(key)
    }
    pub fn save_new(&self, path: &Path, password: &str) -> Result<()> {
        ensure!(
            password.chars().count() >= 8,
            "use a wallet password of at least eight characters"
        );
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        rand08::rngs::OsRng.fill_bytes(&mut salt);
        rand08::rngs::OsRng.fill_bytes(&mut nonce);
        let key = derive_key(password, &salt)?;
        let plaintext = Zeroizing::new(codec::encode(&(self.entropy.as_slice(), self.account))?);
        let aad = codec::encode(&(1u16, self.network, salt, nonce))?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
        let ciphertext = cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("wallet encryption failed"))?;
        let envelope = Envelope {
            version: 1,
            network: self.network,
            salt,
            nonce,
            ciphertext,
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .context("wallet already exists or cannot be created")?;
        file.write_all(&serde_json::to_vec(&envelope)?)?;
        file.sync_all()?;
        Ok(())
    }
    pub fn open(path: &Path, password: &str) -> Result<Self> {
        ensure!(fs::metadata(path)?.len() <= 16_384, "wallet file too large");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                "wallet file permissions are too broad; chmod 600 before opening"
            );
        }
        let envelope: Envelope = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(envelope.version == 1, "unsupported wallet storage version");
        envelope.network.ensure_runnable()?;
        let key = derive_key(password, &envelope.salt)?;
        let aad = codec::encode(&(
            envelope.version,
            envelope.network,
            envelope.salt,
            envelope.nonce,
        ))?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    (&envelope.nonce).into(),
                    Payload {
                        msg: &envelope.ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| anyhow::anyhow!("wrong password or corrupted wallet"))?,
        );
        let (entropy, account): (Vec<u8>, u32) = codec::decode(&plaintext, 16_384)?;
        let entropy = Zeroizing::new(entropy);
        ensure!(entropy.len() == 32, "invalid wallet entropy");
        Ok(Self {
            network: envelope.network,
            entropy,
            account,
        })
    }
}
fn derive_key(password: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    let params =
        Params::new(65_536, 3, 1, Some(32)).map_err(|_| anyhow::anyhow!("KDF parameters"))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut *out)
        .map_err(|_| anyhow::anyhow!("wallet key derivation failed"))?;
    Ok(out)
}
pub fn encode_address(network: Network, address: Address) -> Result<String> {
    Ok(bech32::encode::<bech32::Bech32m>(
        bech32::Hrp::parse(network.hrp())?,
        &address.to_raw_address_bytes(),
    )?)
}
pub fn decode_address(network: Network, text: &str) -> Result<Address> {
    let parsed = bech32::primitives::decode::CheckedHrpstring::new::<bech32::Bech32m>(text)?;
    ensure!(
        parsed.hrp().as_str() == network.hrp(),
        "receiver belongs to a different network"
    );
    let bytes: Vec<_> = parsed.byte_iter().collect();
    let bytes: [u8; 43] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("incorrect receiver length"))?;
    Option::<Address>::from(Address::from_raw_address_bytes(&bytes)).context("invalid receiver")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_seed_recovery_and_network_separation() {
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let phrase = wallet.phrase().unwrap();
        let restored = Wallet::restore(Network::Devnet, &phrase).unwrap();
        assert_eq!(
            wallet.receiving_identity().unwrap(),
            restored.receiving_identity().unwrap()
        );
        assert_eq!(
            wallet.identity_key().unwrap().to_bytes(),
            restored.identity_key().unwrap().to_bytes()
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wallet");
        wallet.save_new(&path, "test-password-only").unwrap();
        assert!(!String::from_utf8_lossy(&fs::read(&path).unwrap()).contains(phrase.as_str()));
        assert!(Wallet::open(&path, "wrong-password").is_err());
        assert_eq!(
            Wallet::open(&path, "test-password-only")
                .unwrap()
                .receiving_identity()
                .unwrap(),
            wallet.receiving_identity().unwrap()
        );
        assert!(decode_address(Network::Testnet, &wallet.receiving_identity().unwrap()).is_err());
        assert!(wallet.save_new(&path, "test-password-only").is_err());
    }
    #[test]
    fn account_and_diversified_receivers_recover_from_seed_and_indices() {
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let account = wallet.for_account(7).unwrap();
        let restored = Wallet::restore(Network::Devnet, &wallet.phrase().unwrap())
            .unwrap()
            .for_account(7)
            .unwrap();
        assert_ne!(
            wallet.receiving_identity().unwrap(),
            account.receiving_identity().unwrap()
        );
        assert_eq!(
            account.receiving_identity().unwrap(),
            restored.receiving_identity().unwrap()
        );
        assert_eq!(account.address(42).unwrap(), restored.address(42).unwrap());
        assert_ne!(account.address(0).unwrap(), account.address(42).unwrap());
        assert_eq!(
            account.identity_key().unwrap().to_bytes(),
            wallet.identity_key().unwrap().to_bytes()
        );
        assert!(wallet.for_account(1u32 << 31).is_err());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("account.wallet");
        account.save_new(&path, "public-test-password").unwrap();
        assert_eq!(
            Wallet::open(&path, "public-test-password").unwrap().account,
            7
        );
    }
}
