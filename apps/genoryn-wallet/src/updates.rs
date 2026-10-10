//! Release notices from GitHub Releases. Only the public "latest release" record is
//! read and nothing about the wallet is sent. Downloads open in the user's browser;
//! the wallet never installs or runs anything it fetched.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::time::Duration;

pub(super) const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub(super) const RELEASES_PAGE: &str = "https://github.com/genoryn/genoryn/releases/latest";
const LATEST_API: &str = "https://api.github.com/repos/genoryn/genoryn/releases/latest";
const RELEASE_PREFIX: &str = "https://github.com/genoryn/genoryn/releases/";
const MAX_RESPONSE: usize = 1_000_000;
const MAX_NOTES: usize = 2_000;
const DOWNLOAD_PREFIX: &str = "https://github.com/genoryn/genoryn/releases/download/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Release {
    pub version: String,
    pub url: String,
    pub notes: String,
    /// Direct download of the Windows installer, when the release has one.
    pub installer: Option<String>,
}
impl Release {
    /// What "Download update" opens: the installer on Windows, else the page.
    pub fn download_url(&self) -> &str {
        if cfg!(target_os = "windows")
            && let Some(installer) = &self.installer
        {
            return installer;
        }
        &self.url
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum UpdateStatus {
    #[default]
    NotChecked,
    Checking,
    UpToDate,
    Available(Release),
    Failed(String),
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}
fn safe_link(url: &str, prefix: &str) -> bool {
    url.starts_with(prefix) && url.bytes().all(|b| b.is_ascii_graphic() && b != b'"')
}
/// The installer of this edition: GENORYN Lite updates only to GENORYN Lite.
pub(super) fn installer_name(lite: bool, version: &str) -> String {
    if lite {
        format!("GENORYN-Lite-Setup-{version}.exe")
    } else {
        format!("GENORYN-Setup-{version}.exe")
    }
}
/// The edition's installer for exactly this version, from this repository.
fn installer_url(assets: &[Asset], lite: bool, version: &str) -> Option<String> {
    let name = installer_name(lite, version);
    assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.clone())
        .filter(|url| safe_link(url, DOWNLOAD_PREFIX) && url.ends_with(&name))
}

/// `v1.2.3`, `1.2.3` and `1.2.3-devnet` all compare as `(1, 2, 3)`.
pub(super) fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// Returns the release only when it is a published, strictly newer version.
pub(super) fn evaluate(current: &str, lite: bool, body: &[u8]) -> Result<Option<Release>> {
    let latest: LatestRelease =
        serde_json::from_slice(body).context("GitHub returned an unexpected release record")?;
    if latest.draft || latest.prerelease {
        return Ok(None);
    }
    let available =
        parse_version(&latest.tag_name).context("the latest release tag is not a version")?;
    let running = parse_version(current).context("this build has an invalid version")?;
    if available <= running {
        return Ok(None);
    }
    // Only ever open this repository's own release pages and downloads.
    let url = if safe_link(&latest.html_url, RELEASE_PREFIX) {
        latest.html_url
    } else {
        RELEASES_PAGE.into()
    };
    let notes = latest
        .body
        .unwrap_or_default()
        .chars()
        .take(MAX_NOTES)
        .collect();
    let version = latest.tag_name.trim_start_matches('v').to_owned();
    Ok(Some(Release {
        installer: installer_url(&latest.assets, lite, &version),
        version,
        url,
        notes,
    }))
}

/// In tor-only mode the request goes only through Tor, which resolves
/// api.github.com; if Tor is unavailable the check fails instead of
/// connecting directly.
pub(super) fn fetch_latest(
    lite: bool,
    route: super::network_privacy::Auxiliary,
) -> Result<Option<Release>> {
    let proxy = super::network_privacy::auxiliary_proxy(&route)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let client = super::network_privacy::http_client(proxy.as_deref().map(String::as_str))?
            .user_agent(concat!("GENORYN-Wallet/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()?;
        let response = client
            .get(LATEST_API)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .context("could not reach GitHub")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure!(
            response.status().is_success(),
            "GitHub returned {}",
            response.status()
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= MAX_RESPONSE as u64),
            "release record is too large"
        );
        let body = response.bytes().await?;
        ensure!(body.len() <= MAX_RESPONSE, "release record is too large");
        evaluate(CURRENT, lite, &body)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// SOCKS5 address type and destination of the one recorded request.
    type Recorded = std::thread::JoinHandle<Option<(u8, Vec<u8>)>>;
    /// A one-shot SOCKS5 stand-in that records the requested destination and
    /// refuses it, so no request leaves this machine.
    fn socks_recorder() -> (std::net::SocketAddr, Recorded) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().ok()?;
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .ok()?;
            let mut head = [0u8; 2];
            stream.read_exact(&mut head).ok()?;
            let mut methods = vec![0u8; head[1] as usize];
            stream.read_exact(&mut methods).ok()?;
            stream.write_all(&[0x05, 0x00]).ok()?;
            let mut request = [0u8; 4];
            stream.read_exact(&mut request).ok()?;
            let destination = match request[3] {
                0x03 => {
                    let mut length = [0u8; 1];
                    stream.read_exact(&mut length).ok()?;
                    let mut name = vec![0u8; length[0] as usize + 2];
                    stream.read_exact(&mut name).ok()?;
                    name.truncate(length[0] as usize);
                    name
                }
                0x01 => {
                    let mut ip = [0u8; 6];
                    stream.read_exact(&mut ip).ok()?;
                    ip.to_vec()
                }
                _ => Vec::new(),
            };
            stream
                .write_all(&[0x05, 0x04, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .ok()?;
            Some((request[3], destination))
        });
        (address, handle)
    }

    #[test]
    fn tor_only_update_checks_reach_tor_by_name_and_fail_closed() {
        use super::super::network_privacy::Auxiliary;
        let (proxy, recorder) = socks_recorder();
        assert!(fetch_latest(false, Auxiliary::Socks(proxy)).is_err());
        let (address_type, destination) = recorder.join().unwrap().expect("proxy contacted");
        assert_eq!(
            address_type, 0x03,
            "socks5h: Tor, not this computer, resolves the name"
        );
        assert_eq!(destination, b"api.github.com");
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let error = format!(
            "{:#}",
            fetch_latest(false, Auxiliary::Socks(closed)).unwrap_err()
        );
        assert!(error.contains("could not reach GitHub"), "{error}");
    }

    fn record(tag: &str, url: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag, "html_url": url, "body": "Notes", "draft": false, "prerelease": false
        }))
        .unwrap()
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("0.10.1-devnet"), Some((0, 10, 1)));
        for bad in ["", "v1", "1.2", "1.2.3.4", "a.b.c", "1.-2.3"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }

    #[test]
    fn only_strictly_newer_published_releases_are_offered() {
        let page = "https://github.com/genoryn/genoryn/releases/tag/v0.3.0";
        let newer = evaluate("0.2.0", false, &record("v0.3.0", page))
            .unwrap()
            .unwrap();
        assert_eq!(newer.version, "0.3.0");
        assert_eq!(newer.url, page);
        assert_eq!(newer.notes, "Notes");
        assert_eq!(
            evaluate("0.2.0", false, &record("v0.2.0", page)).unwrap(),
            None
        );
        assert_eq!(
            evaluate("0.2.0", false, &record("v0.1.9", page)).unwrap(),
            None
        );
        let mut draft: serde_json::Value = serde_json::from_slice(&record("v9.0.0", page)).unwrap();
        draft["prerelease"] = true.into();
        assert_eq!(
            evaluate("0.2.0", false, &serde_json::to_vec(&draft).unwrap()).unwrap(),
            None
        );
        assert!(evaluate("0.2.0", false, b"not json").is_err());
        assert!(evaluate("0.2.0", false, &record("latest", page)).is_err());
    }

    #[test]
    fn each_edition_updates_to_its_own_installer() {
        let page = "https://github.com/genoryn/genoryn/releases/tag/v0.5.0";
        let mut value: serde_json::Value = serde_json::from_slice(&record("v0.5.0", page)).unwrap();
        let download = "https://github.com/genoryn/genoryn/releases/download/v0.5.0/";
        value["assets"] = serde_json::json!([
            {"name": "GENORYN-Setup-0.5.0.exe", "browser_download_url": format!("{download}GENORYN-Setup-0.5.0.exe")},
            {"name": "GENORYN-Lite-Setup-0.5.0.exe", "browser_download_url": format!("{download}GENORYN-Lite-Setup-0.5.0.exe")},
        ]);
        let body = serde_json::to_vec(&value).unwrap();
        let full = evaluate("0.4.1", false, &body).unwrap().unwrap();
        let lite = evaluate("0.4.1", true, &body).unwrap().unwrap();
        assert_eq!(
            full.installer.as_deref(),
            Some(format!("{download}GENORYN-Setup-0.5.0.exe").as_str())
        );
        assert_eq!(
            lite.installer.as_deref(),
            Some(format!("{download}GENORYN-Lite-Setup-0.5.0.exe").as_str())
        );
        value["assets"] = serde_json::json!([
            {"name": "GENORYN-Setup-0.5.0.exe", "browser_download_url": format!("{download}GENORYN-Setup-0.5.0.exe")},
        ]);
        let only_full = evaluate("0.4.1", true, &serde_json::to_vec(&value).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            only_full.installer, None,
            "Lite never installs the miner edition"
        );
    }

    #[test]
    fn installer_link_must_match_version_and_repository() {
        let page = "https://github.com/genoryn/genoryn/releases/tag/v0.3.0";
        let mut value: serde_json::Value = serde_json::from_slice(&record("v0.3.0", page)).unwrap();
        let download = "https://github.com/genoryn/genoryn/releases/download/v0.3.0/";
        value["assets"] = serde_json::json!([
            {"name": "genoryn-0.3.0-devnet-windows-amd64.zip", "browser_download_url": format!("{download}genoryn-0.3.0-devnet-windows-amd64.zip")},
            {"name": "GENORYN-Setup-0.3.0.exe", "browser_download_url": format!("{download}GENORYN-Setup-0.3.0.exe")},
        ]);
        let release = evaluate("0.2.0", false, &serde_json::to_vec(&value).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            release.installer.as_deref(),
            Some(format!("{download}GENORYN-Setup-0.3.0.exe").as_str())
        );
        if cfg!(target_os = "windows") {
            assert!(release.download_url().ends_with(".exe"));
        } else {
            assert_eq!(release.download_url(), page);
        }
        for (name, url) in [
            (
                "GENORYN-Setup-0.2.9.exe",
                format!("{download}GENORYN-Setup-0.2.9.exe"),
            ),
            (
                "GENORYN-Setup-0.3.0.exe",
                "https://evil.example/GENORYN-Setup-0.3.0.exe".to_owned(),
            ),
            ("GENORYN-Setup-0.3.0.exe", format!("{download}other.exe")),
        ] {
            value["assets"] = serde_json::json!([{"name": name, "browser_download_url": url}]);
            let release = evaluate("0.2.0", false, &serde_json::to_vec(&value).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(release.installer, None, "{name} {url}");
            assert_eq!(release.download_url(), page);
        }
    }

    #[test]
    fn foreign_or_malformed_links_fall_back_to_the_release_page() {
        for url in [
            "https://evil.example/download",
            "https://github.com/someone-else/GENORYN/releases/tag/v9.0.0",
            "https://github.com/genoryn/genoryn/releases/tag/v9.0.0\" onclick",
        ] {
            let release = evaluate("0.2.0", false, &record("v9.0.0", url))
                .unwrap()
                .unwrap();
            assert_eq!(release.url, RELEASES_PAGE, "{url}");
        }
    }
}
