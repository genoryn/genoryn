//! Self-update from GitHub releases.
//!
//! A release is installed only if its `SHA256SUMS` carries a valid hybrid
//! ML-DSA-87 + SLH-DSA-SHAKE-256s signature by the project's release key (compiled
//! into this binary) and the downloaded installer matches the signed hash. A
//! compromised download host, CDN or GitHub account cannot push code without that key.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/genoryn/genoryn/releases/latest";
const RELEASE_KEY_HEX: &str = include_str!("../../../docs/release-key.pub");
const CTX: &[u8] = b"genoryn-krypton/release/v1";
const MAX_INSTALLER: u64 = 200 << 20;

#[derive(Clone, Debug, Serialize)]
pub struct Available {
    pub version: String,
    pub page: String,
    /// Installer asset for this platform (None: download manually).
    #[serde(skip)]
    installer: Option<(String, String)>,
    #[serde(skip)]
    sums: Option<String>,
    #[serde(skip)]
    sig: Option<String>,
    pub can_install: bool,
}

/// Strict `[v]MAJOR.MINOR.PATCH`: the version also names the installer file.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim();
    let mut it = v.strip_prefix('v').unwrap_or(v).split('.');
    let mut num = || {
        it.next()
            .filter(|s| !s.is_empty() && s.len() <= 6 && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
    };
    let r = (num()?, num()?, num()?);
    it.next().is_none().then_some(r)
}

/// Whether `candidate` is a newer release than `current`.
pub fn newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "genoryn-krypton-wallet/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(600))
        .build()?)
}

/// The latest release if it is newer than this build.
pub fn check() -> Result<Option<Available>> {
    let v: serde_json::Value = http()?
        .get(LATEST)
        .header("accept", "application/vnd.github+json")
        .send()?
        .error_for_status()?
        .json()?;
    let tag = v["tag_name"].as_str().context("release tag")?.to_string();
    if !newer(&tag, env!("CARGO_PKG_VERSION")) || v["draft"].as_bool() == Some(true) {
        return Ok(None);
    }
    let version = tag.trim().trim_start_matches('v').to_string();
    // The signed checksums name each file with its version, so binding the installer
    // name to the tag stops an old (validly signed) installer being served as an update.
    let installer_name = format!("GenorynKrypton-{version}-Windows-x64-Setup.exe");
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let find = |pred: &dyn Fn(&str) -> bool| {
        assets.iter().find_map(|a| {
            let n = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            pred(n).then(|| (n.to_string(), url.to_string()))
        })
    };
    let installer = if cfg!(windows) {
        find(&|n: &str| n == installer_name)
    } else {
        None
    };
    let sums = find(&|n: &str| n == "SHA256SUMS").map(|x| x.1);
    let sig = find(&|n: &str| n == "SHA256SUMS.sig").map(|x| x.1);
    // Announce nothing that the release key has not signed: someone holding only the
    // GitHub account could otherwise steer users to an unsigned download page.
    let (Some(sums_url), Some(sig_url)) = (sums.clone(), sig.clone()) else {
        return Ok(None);
    };
    let c = http()?;
    let signed = signed_sums(&c, &sums_url, &sig_url)?;
    if !signed.lines().any(|l| l.contains(&format!("-{version}-")) || l.contains(&format!("_{version}_"))) {
        return Ok(None);
    }
    let can_install = installer.is_some();
    Ok(Some(Available {
        page: format!("https://github.com/genoryn/genoryn/releases/tag/v{version}"),
        version,
        installer,
        sums,
        sig,
        can_install,
    }))
}

/// Downloads `SHA256SUMS` and its signature; errors unless the release key signed it.
fn signed_sums(c: &reqwest::blocking::Client, sums_url: &str, sig_url: &str) -> Result<String> {
    let sums = fetch(c, sums_url, 1 << 20)?;
    let sig_hex = fetch(c, sig_url, 1 << 20)?;
    let key = hex::decode(RELEASE_KEY_HEX.trim()).context("embedded release key")?;
    let sig =
        hex::decode(String::from_utf8_lossy(&sig_hex).trim()).context("signature encoding")?;
    if !krypton_crypto::sign::hybrid_verify(&key, &sums, &sig, CTX) {
        bail!("release signature is invalid: update refused");
    }
    Ok(String::from_utf8_lossy(&sums).into_owned())
}

fn fetch(c: &reqwest::blocking::Client, url: &str, max: u64) -> Result<Vec<u8>> {
    ensure!(
        url.starts_with("https://github.com/genoryn/genoryn/"),
        "unexpected download location"
    );
    let r = c.get(url).send()?.error_for_status()?;
    if let Some(len) = r.content_length() {
        ensure!(len <= max, "download too large");
    }
    // Bounded even without a Content-Length (chunked responses).
    let mut b = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(r, max + 1), &mut b)?;
    ensure!(b.len() as u64 <= max, "download too large");
    Ok(b)
}

/// Downloads the installer and verifies the signed checksums. Returns its path.
pub fn download_verified(a: &Available, dir: &Path) -> Result<PathBuf> {
    let (name, url) = a
        .installer
        .clone()
        .context("no installer for this platform")?;
    ensure!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
        "unexpected installer name"
    );
    let c = http()?;
    let sums = signed_sums(
        &c,
        a.sums.as_deref().context("SHA256SUMS missing")?,
        a.sig.as_deref().context("SHA256SUMS.sig missing")?,
    )?;
    let want = sums
        .lines()
        .find_map(|l| {
            let (h, n) = l.split_once("  ")?;
            (n.trim() == name).then(|| h.trim().to_lowercase())
        })
        .context("installer not listed in the signed checksums")?;
    let bin = fetch(&c, &url, MAX_INSTALLER)?;
    let got = hex::encode(Sha256::digest(&bin));
    ensure!(
        got == want,
        "installer hash does not match the signed checksums"
    );
    // A fresh, unpredictable folder: nothing can be planted at the path in advance.
    let dir = dir.join(hex::encode(rand::random::<[u8; 12]>()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&name);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(&bin)?;
        f.sync_all()?;
    }
    // Hash what is on disk right before it runs.
    let disk = std::fs::read(&path)?;
    ensure!(
        hex::encode(Sha256::digest(&disk)) == want,
        "installer changed on disk: update refused"
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_order() {
        assert!(newer("v1.0.1", "1.0.0"));
        assert!(newer("1.1.0", "1.0.9"));
        assert!(newer("v2.0.0", "1.99.99"));
        assert!(!newer("v1.0.0", "1.0.0"));
        assert!(!newer("v0.9.9", "1.0.0"));
        assert!(!newer("garbage", "1.0.0"));
        assert!(!newer("v9.0.0-/../../x", "1.0.0"), "strict tags only");
        assert!(!newer("v9.0.0.1", "1.0.0"));
        assert!(!newer("v9..0", "1.0.0"));
    }
    #[test]
    fn embedded_key_is_valid_hex() {
        assert!(hex::decode(RELEASE_KEY_HEX.trim()).unwrap().len() > 1000);
    }
}
