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
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(&env)?)?;
            // On disk before the rename, so a power cut leaves the old file or the new one.
            f.sync_all()?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(path: &Path, f: impl FnOnce(&mut serde_json::Value)) {
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        f(&mut v);
        std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    }

    #[test]
    fn only_the_password_opens_the_file() {
        let dir = std::env::temp_dir().join(format!("gk-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("w.json");
        let secret = b"24 words and every note";
        FileKey::new("correct horse battery staple", Argon2Params::TEST)
            .unwrap()
            .save(&path, secret)
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("every note"), "plaintext on disk");
        let (_, pt) = FileKey::open(&path, "correct horse battery staple").unwrap();
        assert_eq!(&pt[..], secret);
        assert!(FileKey::open(&path, "correct horse battery stapl").is_err());
        assert!(FileKey::open(&path, "").is_err());

        // A copy elsewhere opens with the password and nothing else.
        let copy = dir.join("copy.json");
        std::fs::copy(&path, &copy).unwrap();
        assert!(FileKey::open(&copy, "correct horse battery staple").is_ok());
        assert!(FileKey::open(&copy, "guess").is_err());

        // Any changed byte of the ciphertext, salt, nonce or parameters is refused.
        for field in ["ciphertext", "salt", "nonce"] {
            std::fs::copy(&path, &copy).unwrap();
            edit(&copy, |v| {
                let s = v[field].as_str().unwrap().to_string();
                let flip = if s.starts_with('0') { "1" } else { "0" };
                v[field] = format!("{flip}{}", &s[1..]).into();
            });
            assert!(
                FileKey::open(&copy, "correct horse battery staple").is_err(),
                "{field}"
            );
        }
        std::fs::copy(&path, &copy).unwrap();
        edit(&copy, |v| {
            v["kdf"]["t_cost"] = (v["kdf"]["t_cost"].as_u64().unwrap() + 1).into()
        });
        assert!(
            FileKey::open(&copy, "correct horse battery staple").is_err(),
            "kdf params"
        );
        // Weakening the parameters below the floor is refused before any work.
        std::fs::copy(&path, &copy).unwrap();
        edit(&copy, |v| v["kdf"]["m_cost_kib"] = 8.into());
        assert!(FileKey::open(&copy, "correct horse battery staple").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
