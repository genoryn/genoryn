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
/// Argon2id cost of one wallet file. Guessing a password costs an attacker
/// this much memory and time per guess, on any hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}
impl KdfParams {
    /// Wallet files of version 1 (GENORYN 0.x to 1.1).
    pub const V1: Self = Self {
        memory_kib: 65_536,
        passes: 3,
        lanes: 1,
    };
    /// Wallet files written since 1.2: four times the memory of version 1.
    pub const CURRENT: Self = Self {
        memory_kib: 262_144,
        passes: 3,
        lanes: 1,
    };
    /// Bounds a file may ask for, so a crafted file cannot exhaust memory.
    fn check(self) -> Result<()> {
        ensure!(
            (Self::V1.memory_kib..=1_048_576).contains(&self.memory_kib)
                && (1..=10).contains(&self.passes)
                && (1..=4).contains(&self.lanes),
            "unsupported wallet key-derivation parameters"
        );
        Ok(())
    }
}
/// What a wallet file's unencrypted header says about its protection.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StorageInfo {
    pub version: u16,
    pub network: Network,
    pub kdf: KdfParams,
}
impl StorageInfo {
    /// Whether the file already uses at least the current key-derivation cost.
    pub fn is_current(&self) -> bool {
        self.kdf.memory_kib >= KdfParams::CURRENT.memory_kib
            && self.kdf.passes >= KdfParams::CURRENT.passes
    }
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u16,
    network: Network,
    /// Present from version 2; version 1 implies `KdfParams::V1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kdf: Option<KdfParams>,
    salt: [u8; 16],
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}
impl Envelope {
    fn read(path: &Path) -> Result<Self> {
        ensure!(fs::metadata(path)?.len() <= 16_384, "wallet file too large");
        let envelope: Self =
            serde_json::from_slice(&fs::read(path)?).context("not a GENORYN wallet file")?;
        match (envelope.version, envelope.kdf) {
            (1, None) => {}
            (2, Some(kdf)) => kdf.check()?,
            _ => anyhow::bail!("unsupported wallet storage version"),
        }
        Ok(envelope)
    }
    fn kdf(&self) -> KdfParams {
        self.kdf.unwrap_or(KdfParams::V1)
    }
    /// Authenticated header: changing the version, network, salt, nonce or
    /// cost parameters makes decryption fail.
    fn aad(&self) -> Result<Vec<u8>> {
        Ok(match self.kdf {
            None => codec::encode(&(self.version, self.network, self.salt, self.nonce))?,
            Some(kdf) => codec::encode(&(
                self.version,
                self.network,
                self.salt,
                self.nonce,
                kdf.memory_kib,
                kdf.passes,
                kdf.lanes,
            ))?,
        })
    }
}
/// The protection of the wallet file at `path`, read without its password.
pub fn storage_info(path: &Path) -> Result<StorageInfo> {
    let envelope = Envelope::read(path)?;
    Ok(StorageInfo {
        version: envelope.version,
        network: envelope.network,
        kdf: envelope.kdf(),
    })
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
    /// Key for one kind of local wallet storage (`domain`), bound to network and
    /// account; distinct from every spending/viewing key and from other domains.
    pub fn storage_key(&self, domain: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        let seed = self.seed()?;
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(domain), &*seed);
        let mut key = Zeroizing::new([0u8; 32]);
        let mut info = vec![self.network.id()];
        info.extend_from_slice(&self.account.to_le_bytes());
        hk.expand(&info, &mut *key)
            .map_err(|_| anyhow::anyhow!("storage key derivation failed"))?;
        Ok(key)
    }
    pub fn save_new(&self, path: &Path, password: &str) -> Result<()> {
        self.save_new_with(path, password, KdfParams::CURRENT)
    }
    /// Writes a new wallet file; `kdf` other than `CURRENT` is for tests and
    /// for reproducing older files.
    pub fn save_new_with(&self, path: &Path, password: &str, kdf: KdfParams) -> Result<()> {
        ensure!(
            password.chars().count() >= 8,
            "use a wallet password of at least eight characters"
        );
        kdf.check()?;
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        rand08::rngs::OsRng.fill_bytes(&mut salt);
        rand08::rngs::OsRng.fill_bytes(&mut nonce);
        let mut envelope = Envelope {
            version: if kdf == KdfParams::V1 { 1 } else { 2 },
            network: self.network,
            kdf: (kdf != KdfParams::V1).then_some(kdf),
            salt,
            nonce,
            ciphertext: Vec::new(),
        };
        let key = derive_key(password, &salt, kdf)?;
        let plaintext = Zeroizing::new(codec::encode(&(self.entropy.as_slice(), self.account))?);
        let aad = envelope.aad()?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
        envelope.ciphertext = cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("wallet encryption failed"))?;
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                "wallet file permissions are too broad; chmod 600 before opening"
            );
        }
        let envelope = Envelope::read(path)?;
        envelope.network.ensure_runnable()?;
        let key = derive_key(password, &envelope.salt, envelope.kdf())?;
        let aad = envelope.aad()?;
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
    /// Re-encrypts the wallet file at `path` under `new_password` with the
    /// current key-derivation cost (the same password strengthens the file).
    /// The new file is written beside the old one and opened again with the
    /// new password; only when it holds the same keys does it replace the
    /// original, so a failure at any step leaves the original untouched.
    pub fn change_password(path: &Path, old_password: &str, new_password: &str) -> Result<Self> {
        let wallet = Self::open(path, old_password)?;
        let mut tag = [0u8; 8];
        rand08::rngs::OsRng.fill_bytes(&mut tag);
        let file_name = path
            .file_name()
            .context("wallet path has no file name")?
            .to_string_lossy();
        let temporary = path.with_file_name(format!(".{file_name}.{}.rekey", hex::encode(tag)));
        let replaced = (|| -> Result<()> {
            wallet.save_new_with(&temporary, new_password, KdfParams::CURRENT)?;
            let check = Self::open(&temporary, new_password)?;
            ensure!(
                check.network == wallet.network
                    && check.account == wallet.account
                    && *check.entropy == *wallet.entropy,
                "the re-encrypted wallet did not match; the original file was kept"
            );
            fs::rename(&temporary, path).context("could not replace the wallet file")?;
            #[cfg(unix)]
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        if replaced.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        replaced.map(|()| wallet)
    }
}
fn derive_key(password: &str, salt: &[u8; 16], kdf: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(kdf.memory_kib, kdf.passes, kdf.lanes, Some(32))
        .map_err(|_| anyhow::anyhow!("KDF parameters"))?;
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
    fn version_1_files_open_and_upgrade_only_through_a_checked_password_change() {
        let wallet = Wallet::create(Network::Devnet)
            .unwrap()
            .for_account(3)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.gnr");
        wallet
            .save_new_with(&path, "old-password-1", KdfParams::V1)
            .unwrap();
        let info = storage_info(&path).unwrap();
        assert_eq!((info.version, info.kdf), (1, KdfParams::V1));
        assert!(!info.is_current());
        let before = fs::read(&path).unwrap();
        // A wrong current password changes nothing on disk.
        assert!(Wallet::change_password(&path, "not-the-password", "new-password-22").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let changed = Wallet::change_password(&path, "old-password-1", "new-password-22").unwrap();
        assert_eq!(
            changed.receiving_identity().unwrap(),
            wallet.receiving_identity().unwrap()
        );
        let info = storage_info(&path).unwrap();
        assert_eq!((info.version, info.kdf), (2, KdfParams::CURRENT));
        assert!(info.is_current());
        assert!(Wallet::open(&path, "old-password-1").is_err());
        let reopened = Wallet::open(&path, "new-password-22").unwrap();
        assert_eq!(reopened.account, 3);
        assert_eq!(reopened.phrase().unwrap(), wallet.phrase().unwrap());
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("old.gnr")]);
    }
    #[test]
    fn cost_parameters_are_authenticated_and_bounded() {
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.gnr");
        wallet.save_new(&path, "strong-password-77").unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let rewrite = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value = original.clone();
            change(&mut value);
            let tampered = dir.path().join("tampered.gnr");
            let _ = fs::remove_file(&tampered);
            fs::write(&tampered, serde_json::to_vec(&value).unwrap()).unwrap();
            Wallet::open(&tampered, "strong-password-77").map(|_| ())
        };
        // Lowering the cost in the header does not make the file cheaper to open.
        let weaker = rewrite(&|v| v["kdf"]["memory_kib"] = 131_072.into()).unwrap_err();
        assert!(
            weaker.to_string().contains("wrong password or corrupted"),
            "{weaker}"
        );
        assert!(rewrite(&|v| v["kdf"]["memory_kib"] = 1u64.into()).is_err());
        let huge = rewrite(&|v| v["kdf"]["memory_kib"] = 4_000_000_000u64.into()).unwrap_err();
        assert!(huge.to_string().contains("unsupported"), "{huge}");
        // A version 2 file without its parameters, or a version 1 file with some, is refused.
        assert!(
            rewrite(&|v| {
                v.as_object_mut().unwrap().remove("kdf");
            })
            .is_err()
        );
        assert!(rewrite(&|v| v["version"] = 1.into()).is_err());
        assert!(rewrite(&|_| {}).is_ok());
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
