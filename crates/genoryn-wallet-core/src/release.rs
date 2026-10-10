//! Explicit trust-root release verification. Never executes an update or creates keys.
use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Deserialize)]
pub struct Manifest {
    pub format_version: u16,
    pub project: String,
    pub version: String,
    pub files: Vec<ReleaseFile>,
}
#[derive(Deserialize)]
pub struct ReleaseFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
pub fn signed_message(bytes: &[u8]) -> Vec<u8> {
    [b"GENORYN/release-manifest/v1\0".as_slice(), bytes].concat()
}
pub fn verify_metadata(bytes: &[u8], signature: &[u8], trusted_key: [u8; 32]) -> Result<Manifest> {
    ensure!(bytes.len() <= 2_000_000, "release manifest too large");
    VerifyingKey::from_bytes(&trusted_key)?
        .verify_strict(&signed_message(bytes), &Signature::from_slice(signature)?)?;
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    ensure!(
        manifest.format_version == 1 && manifest.project == "GENORYN",
        "unsupported release manifest"
    );
    ensure!(
        !manifest.files.is_empty() && manifest.files.len() <= 10_000,
        "manifest file count"
    );
    let mut names = std::collections::BTreeSet::new();
    for file in &manifest.files {
        ensure!(
            !file.name.is_empty()
                && !file.name.contains(['\\', ':', '\0'])
                && !Path::new(&file.name).is_absolute()
                && Path::new(&file.name)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "unsafe release path"
        );
        ensure!(names.insert(&file.name), "duplicate release file");
        ensure!(
            file.sha256.len() == 64 && hex::decode(&file.sha256).is_ok(),
            "invalid checksum"
        );
    }
    Ok(manifest)
}
pub fn verify_files(manifest: &Manifest, directory: &Path) -> Result<()> {
    let root = directory.canonicalize()?;
    for file in &manifest.files {
        let path = root
            .join(&file.name)
            .canonicalize()
            .with_context(|| format!("missing release file {}", file.name))?;
        ensure!(
            path.starts_with(&root),
            "release symlink leaves package directory"
        );
        ensure!(
            std::fs::metadata(&path)?.len() == file.size,
            "release file size mismatch: {}",
            file.name
        );
        let mut reader = std::fs::File::open(&path)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65_536];
        use std::io::Read;
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        ensure!(
            hex::encode(hash.finalize()) == file.sha256,
            "release file checksum mismatch: {}",
            file.name
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn trusted_key_and_exact_contents_required() {
        let key = SigningKey::from_bytes(&[17; 32]); // Publicly known test-only key; never a release identity.
        let content = b"verified test binary";
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app"), content).unwrap();
        let bytes=serde_json::to_vec(&serde_json::json!({"format_version":1,"project":"GENORYN","version":"0.1.0-test","files":[{"name":"app","size":content.len(),"sha256":hex::encode(Sha256::digest(content))}]})).unwrap();
        let sig = key.sign(&signed_message(&bytes));
        let m = verify_metadata(&bytes, &sig.to_bytes(), key.verifying_key().to_bytes()).unwrap();
        verify_files(&m, dir.path()).unwrap();
        let mut tampered = bytes.clone();
        tampered.push(b' ');
        assert!(
            verify_metadata(&tampered, &sig.to_bytes(), key.verifying_key().to_bytes()).is_err()
        );
        assert!(
            verify_metadata(
                &bytes,
                &sig.to_bytes(),
                SigningKey::from_bytes(&[18; 32]).verifying_key().to_bytes()
            )
            .is_err()
        );
        std::fs::write(dir.path().join("app"), b"malicious replacement").unwrap();
        assert!(verify_files(&m, dir.path()).is_err());
    }
}
