//! Encrypted wallet file: Argon2id (256 MiB) key derivation, XChaCha20-Poly1305,
//! parameters authenticated as associated data, atomic replace on write.

use anyhow::{Context, Result, bail};
use krypton_crypto::aead;
use krypton_crypto::kdf::{Argon2Params, argon2id};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::Zeroizing;

const MAGIC: &str = "genoryn-krypton-wallet";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope {
    magic: String,
    version: u32,
    kdf: Argon2Params,
    #[serde(with = "hex::serde")]
    salt: [u8; 32],
    #[serde(with = "hex::serde")]
    nonce: [u8; 24],
    #[serde(with = "hex::serde")]
    ciphertext: Vec<u8>,
}

fn aad(kdf: &Argon2Params, salt: &[u8; 32]) -> Vec<u8> {
    format!(
        "{MAGIC}|{VERSION}|{}|{}|{}|{}",
        kdf.m_cost_kib,
        kdf.t_cost,
        kdf.p_cost,
        hex::encode(salt)
    )
    .into_bytes()
}

/// Derived file key kept in memory while the wallet is open, so saving does not
/// need the password again.
pub struct FileKey {
    key: Zeroizing<[u8; 32]>,
    kdf: Argon2Params,
    salt: [u8; 32],
}

impl FileKey {
    pub fn new(password: &str, kdf: Argon2Params) -> Result<Self> {
        let mut salt = [0u8; 32];
        rand_core::OsRng.fill_bytes(&mut salt);
        Ok(Self {
            key: argon2id(password.as_bytes(), &salt, kdf)?,
            kdf,
            salt,
        })
    }

    pub fn save(&self, path: &Path, plaintext: &[u8]) -> Result<()> {
        let mut nonce = [0u8; 24];
        rand_core::OsRng.fill_bytes(&mut nonce);
        let env = Envelope {
            magic: MAGIC.into(),
            version: VERSION,
            kdf: self.kdf,
            salt: self.salt,
            nonce,
            ciphertext: aead::seal(&self.key, &nonce, &aad(&self.kdf, &self.salt), plaintext),
        };
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&env)?)?;
        std::fs::rename(&tmp, path).context("replace wallet file")?;
        Ok(())
    }

    pub fn open(path: &Path, password: &str) -> Result<(Self, Zeroizing<Vec<u8>>)> {
        let env: Envelope =
            serde_json::from_slice(&std::fs::read(path).context("read wallet file")?)?;
        if env.magic != MAGIC || env.version != VERSION {
            bail!("not a Genoryn Krypton wallet file");
        }
        if env.kdf.m_cost_kib < 1024 || env.kdf.m_cost_kib > 4 * 1024 * 1024 || env.kdf.t_cost > 64
        {
            bail!("unreasonable key derivation parameters");
        }
        let key = argon2id(password.as_bytes(), &env.salt, env.kdf)?;
        let pt = aead::open(&key, &env.nonce, &aad(&env.kdf, &env.salt), &env.ciphertext)
            .map_err(|_| anyhow::anyhow!("wrong password or damaged wallet file"))?;
        Ok((
            Self {
                key,
                kdf: env.kdf,
                salt: env.salt,
            },
            Zeroizing::new(pt),
        ))
    }
}
