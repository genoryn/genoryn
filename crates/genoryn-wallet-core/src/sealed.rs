//! Authenticated, encrypted local files beside a wallet (scan cache, outbox,
//! verified chain). Each file kind has its own key and is bound to the wallet's
//! network, genesis block and account, so a file from another wallet, network
//! or chain never decrypts. Writes are atomic: a crash leaves the old or the
//! new file, never a mix.
use crate::keys::Wallet;
use anyhow::{Context, Result, ensure};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305,
    aead::{Aead, Payload},
};
use genoryn_core::{codec, params::Network};
use rand08::RngCore;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u16,
    network: Network,
    account: u32,
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

fn aad(domain: &[u8], network: Network, account: u32, nonce: &[u8; 24]) -> Result<Vec<u8>> {
    codec::encode(&(domain, network, network.genesis(), account, nonce))
}

/// Reads `path` sealed under `domain`. `Ok(None)` when the file does not exist;
/// an error when it exists but is foreign, tampered or too large.
pub fn load<T: Serialize + DeserializeOwned>(
    wallet: &Wallet,
    path: &Path,
    domain: &[u8],
    version: u16,
    max_bytes: u64,
) -> Result<Option<T>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(metadata.len() <= max_bytes, "local wallet file too large");
    let envelope: Envelope = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        envelope.version == version
            && envelope.network == wallet.network
            && envelope.account == wallet.account,
        "local wallet file belongs to another wallet, network or version"
    );
    let key = wallet.storage_key(domain)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
    let plaintext = zeroize::Zeroizing::new(
        cipher
            .decrypt(
                (&envelope.nonce).into(),
                Payload {
                    msg: &envelope.ciphertext,
                    aad: &aad(domain, envelope.network, envelope.account, &envelope.nonce)?,
                },
            )
            .map_err(|_| anyhow::anyhow!("local wallet file does not belong to this wallet"))?,
    );
    Ok(Some(codec::decode(&plaintext, max_bytes as usize)?))
}

/// Writes `value` sealed under `domain`, durably (data synced before rename).
pub fn save<T: Serialize>(
    wallet: &Wallet,
    path: &Path,
    domain: &[u8],
    version: u16,
    value: &T,
) -> Result<()> {
    let mut nonce = [0u8; 24];
    rand08::rngs::OsRng.fill_bytes(&mut nonce);
    let key = wallet.storage_key(domain)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| anyhow::anyhow!("cipher init"))?;
    let plaintext = zeroize::Zeroizing::new(codec::encode(value)?);
    let ciphertext = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: &plaintext,
                aad: &aad(domain, wallet.network, wallet.account, &nonce)?,
            },
        )
        .map_err(|_| anyhow::anyhow!("local wallet file encryption failed"))?;
    let envelope = Envelope {
        version,
        network: wallet.network,
        account: wallet.account,
        nonce,
        ciphertext,
    };
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = std::path::PathBuf::from(temporary);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&envelope)?)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path).context("could not store a local wallet file")?;
    #[cfg(unix)]
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_files_bind_domain_wallet_and_detect_tampering() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("w.test");
        let wallet = Wallet::create(Network::Devnet).unwrap();
        assert_eq!(
            load::<Vec<u64>>(&wallet, &path, b"A", 1, 1000).unwrap(),
            None,
            "a missing file is not an error"
        );
        save(&wallet, &path, b"A", 1, &vec![1u64, 2, 3]).unwrap();
        assert_eq!(
            load::<Vec<u64>>(&wallet, &path, b"A", 1, 1000).unwrap(),
            Some(vec![1, 2, 3])
        );
        assert!(load::<Vec<u64>>(&wallet, &path, b"B", 1, 1000).is_err());
        assert!(load::<Vec<u64>>(&wallet, &path, b"A", 2, 1000).is_err());
        let other = Wallet::create(Network::Devnet).unwrap();
        assert!(load::<Vec<u64>>(&other, &path, b"A", 1, 1000).is_err());
        let mut text = std::fs::read_to_string(&path).unwrap();
        let at = text.find("\"ciphertext\":[").unwrap() + 14;
        let digit = if &text[at..at + 1] == "1" { "2" } else { "1" };
        text.replace_range(at..at + 1, digit);
        std::fs::write(&path, text).unwrap();
        assert!(load::<Vec<u64>>(&wallet, &path, b"A", 1, 1000).is_err());
    }
}
