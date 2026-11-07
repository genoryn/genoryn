//! krypton-release: post-quantum release signing.
//!
//!   krypton-release keygen <secret-seed-file> <public-key-file>
//!   krypton-release sums <dir>                          -> writes <dir>/SHA256SUMS
//!   krypton-release sign <secret-seed-file> <SHA256SUMS> -> writes SHA256SUMS.sig
//!     (`-` as the seed file reads the hex seed from standard input, so it never has to
//!     be written to disk)
//!   krypton-release verify <public-key-file> <SHA256SUMS> <SHA256SUMS.sig>
//!
//! A signature is valid only if BOTH ML-DSA-87 (FIPS 204) and SLH-DSA-SHAKE-256s
//! (FIPS 205, hash-based) verify.

use anyhow::{Context, Result, bail};
use krypton_crypto::sign::{HybridKey, hybrid_verify};
use rand_core::RngCore;
use sha2::{Digest, Sha256};
use std::path::Path;

const CTX: &[u8] = b"genoryn-krypton/release/v1";

fn read_hex(p: &str) -> Result<Vec<u8>> {
    let s = if p == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        s
    } else {
        std::fs::read_to_string(p).with_context(|| p.to_string())?
    };
    // A byte-order mark is not part of the key (Windows PowerShell adds one to pipes).
    Ok(hex::decode(s.trim().trim_start_matches('\u{feff}').trim())?)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("keygen") if a.len() == 3 => {
            if Path::new(&a[1]).exists() {
                bail!("{} exists; refusing to overwrite a signing key", a[1]);
            }
            let mut seed = [0u8; 128];
            rand_core::OsRng.fill_bytes(&mut seed);
            let k = HybridKey::from_seed(&seed);
            std::fs::write(&a[1], hex::encode(seed))?;
            std::fs::write(&a[2], hex::encode(k.public_bytes()) + "\n")?;
            println!(
                "public key fingerprint {}",
                &hex::encode(Sha256::digest(k.public_bytes()))[..16]
            );
        }
        Some("sums") if a.len() == 2 => {
            let dir = Path::new(&a[1]);
            let mut names: Vec<String> = std::fs::read_dir(dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with("SHA256SUMS"))
                .collect();
            names.sort();
            let mut out = String::new();
            for n in names {
                let h = Sha256::digest(std::fs::read(dir.join(&n))?);
                out += &format!("{}  {}\n", hex::encode(h), n);
            }
            std::fs::write(dir.join("SHA256SUMS"), &out)?;
            print!("{out}");
        }
        Some("sign") if a.len() == 3 => {
            let seed: [u8; 128] = read_hex(&a[1])?
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad seed"))?;
            let k = HybridKey::from_seed(&seed);
            let msg = std::fs::read(&a[2])?;
            let sig = k.sign(&msg, CTX).map_err(|e| anyhow::anyhow!("{e}"))?;
            assert!(hybrid_verify(&k.public_bytes(), &msg, &sig, CTX));
            println!("signed {} ({} bytes of signature)", a[2], sig.len());
            std::fs::write(format!("{}.sig", a[2]), hex::encode(sig) + "\n")?;
        }
        Some("verify") if a.len() == 4 => {
            let pk = read_hex(&a[1])?;
            let msg = std::fs::read(&a[2])?;
            let sig = read_hex(&a[3])?;
            if hybrid_verify(&pk, &msg, &sig, CTX) {
                println!("OK: valid ML-DSA-87 + SLH-DSA-SHAKE-256s signature");
            } else {
                bail!("INVALID signature");
            }
        }
        _ => bail!("usage: krypton-release keygen|sums|sign|verify ..."),
    }
    Ok(())
}
