//! Read-only GENORYN endpoint discovery. Endpoint reports are not consensus proofs.
//!
//! Applications pin a chain ID and genesis hash from trusted configuration. Bootstrap
//! URLs, DNS TXT records and signed manifests only identify candidates; every candidate
//! must pass the same health policy. No production bootstrap authority is hardcoded.
#![forbid(unsafe_code)]

use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinSet;

const MAX_CANDIDATES: usize = 32;
const MAX_BODY: usize = 16 * 1024 * 1024;
const MANIFEST_DOMAIN: &[u8] = b"GENORYN bootstrap manifest v1\0";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectConfig {
    pub network: String,
    pub chain_id: String,
    pub genesis_hash: String,
    /// Explicit user/operator configuration. Empty is allowed, and fails closed.
    pub bootstrap_endpoints: Vec<String>,
    /// Fully qualified TXT names. TXT value format: `genoryn-rpc-v1=https://host`.
    #[serde(default)]
    pub dns_seeds: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_skew")]
    pub max_height_skew: u64,
}
fn default_timeout() -> u64 {
    3_000
}
fn default_skew() -> u64 {
    10
}

impl ConnectConfig {
    pub fn devnet(
        chain_id: impl Into<String>,
        genesis_hash: impl Into<String>,
        endpoints: Vec<String>,
    ) -> Self {
        Self::for_network("devnet", chain_id, genesis_hash, endpoints)
    }
    pub fn for_network(
        network: impl Into<String>,
        chain_id: impl Into<String>,
        genesis_hash: impl Into<String>,
        endpoints: Vec<String>,
    ) -> Self {
        Self {
            network: network.into(),
            chain_id: chain_id.into(),
            genesis_hash: genesis_hash.into(),
            bootstrap_endpoints: endpoints,
            dns_seeds: Vec::new(),
            timeout_ms: default_timeout(),
            max_height_skew: default_skew(),
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            ["devnet", "testnet", "mainnet"].contains(&self.network.as_str()),
            "unsupported network"
        );
        ensure!(
            !self.chain_id.is_empty() && self.chain_id.len() <= 128,
            "invalid expected chain ID"
        );
        ensure!(
            self.genesis_hash.len() == 64 && hex::decode(&self.genesis_hash).is_ok(),
            "expected genesis hash must contain 32 hex bytes"
        );
        ensure!(
            (100..=30_000).contains(&self.timeout_ms),
            "timeout must be 100–30000 ms"
        );
        ensure!(
            self.bootstrap_endpoints.len() <= MAX_CANDIDATES && self.dns_seeds.len() <= 8,
            "too many discovery sources"
        );
        for endpoint in &self.bootstrap_endpoints {
            validate_endpoint(endpoint, &self.network)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub network: String,
    pub chain_id: String,
    pub genesis_hash: String,
    pub version: u32,
    pub height: u64,
    pub best_hash: String,
    pub difficulty: String,
    pub peer_count: u64,
    #[serde(default)]
    pub inbound_peers: Option<u64>,
    #[serde(default)]
    pub outbound_peers: Option<u64>,
    #[serde(default)]
    pub database_bytes: Option<u64>,
    #[serde(default)]
    pub estimated_hashrate_hs: Option<f64>,
    #[serde(default)]
    pub hashrate_estimate_basis: Option<String>,
    pub mempool_count: u64,
    pub public_rpc: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthyNode {
    pub endpoint: String,
    pub latency_ms: u64,
    pub info: NodeInfo,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub healthy: Vec<HealthyNode>,
    pub rejected: Vec<RejectedNode>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RejectedNode {
    pub endpoint: String,
    pub reason: String,
}

/// A signature authenticates discovery hints, never block validity or a consensus authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapManifest {
    pub version: u32,
    pub network: String,
    pub chain_id: String,
    pub genesis_hash: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub endpoints: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifest {
    pub payload: BootstrapManifest,
    pub signature: String,
}

impl BootstrapManifest {
    /// Domain-separated UTF-8 serde JSON in this struct's declared field order.
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = MANIFEST_DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(self)?);
        Ok(bytes)
    }
}
impl SignedManifest {
    /// `trusted_key` must come from trusted app/operator configuration, never the manifest.
    pub fn verify(
        &self,
        trusted_key: &[u8; 32],
        config: &ConnectConfig,
        now: u64,
    ) -> Result<Vec<String>> {
        config.validate()?;
        ensure!(self.payload.version == 1, "unsupported manifest version");
        ensure!(
            self.payload.network == config.network
                && self.payload.chain_id == config.chain_id
                && self.payload.genesis_hash == config.genesis_hash,
            "manifest chain identity mismatch"
        );
        ensure!(
            self.payload.issued_at <= now && now < self.payload.expires_at,
            "manifest not yet valid or expired"
        );
        ensure!(
            self.payload
                .expires_at
                .saturating_sub(self.payload.issued_at)
                <= 30 * 86_400,
            "manifest lifetime exceeds 30 days"
        );
        ensure!(
            !self.payload.endpoints.is_empty() && self.payload.endpoints.len() <= MAX_CANDIDATES,
            "invalid manifest endpoint count"
        );
        let key = VerifyingKey::from_bytes(trusted_key).context("invalid trusted manifest key")?;
        let sig = Signature::from_slice(
            &hex::decode(&self.signature).context("invalid signature encoding")?,
        )?;
        key.verify_strict(&self.payload.signing_bytes()?, &sig)
            .context("bootstrap signature rejected")?;
        for endpoint in &self.payload.endpoints {
            validate_endpoint(endpoint, &config.network)?;
        }
        Ok(self.payload.endpoints.clone())
    }
}

/// Validate the scheme, authority and base path before any network request.
/// Plain HTTP is permitted only for literal loopback IPs: the user's own node on the
/// same machine, whose traffic never leaves it. Every remote endpoint needs HTTPS.
pub fn validate_endpoint(endpoint: &str, network: &str) -> Result<Url> {
    ensure!(endpoint.len() <= 2_048, "endpoint too long");
    let mut url = Url::parse(endpoint).context("invalid endpoint URL")?;
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "endpoint credentials forbidden"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "endpoint query/fragment forbidden"
    );
    ensure!(
        url.path() == "/" || url.path().is_empty(),
        "endpoint must be an origin without a path"
    );
    ensure!(url.host_str().is_some(), "endpoint host required");
    // This client has no Tor transport; resolving an onion name through the
    // system resolver would leak it to the local network and DNS servers.
    ensure!(
        !url.host_str().is_some_and(|host| host
            .trim_end_matches('.')
            .to_ascii_lowercase()
            .ends_with(".onion")),
        ".onion endpoints need Tor routing, which this client does not provide; refusing to resolve one through system DNS"
    );
    ensure!(
        ["devnet", "testnet", "mainnet"].contains(&network),
        "unsupported network"
    );
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && is_loopback(&url)),
        "HTTPS required except literal-loopback HTTP to your own node"
    );
    url.set_path("");
    Ok(url)
}
fn is_loopback(url: &Url) -> bool {
    url.host_str()
        .and_then(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback())
}

#[derive(Clone)]
pub struct Connect {
    config: ConnectConfig,
    http: Client,
    candidates: BTreeSet<String>,
    last_report: DiscoveryReport,
}
impl Connect {
    pub fn new(config: ConnectConfig) -> Result<Self> {
        config.validate()?;
        let candidates = config
            .bootstrap_endpoints
            .iter()
            .map(|s| s.trim_end_matches('/').to_owned())
            .collect();
        let http = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .connect_timeout(Duration::from_millis(config.timeout_ms))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("genoryn-connect/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            config,
            http,
            candidates,
            last_report: DiscoveryReport::default(),
        })
    }
    pub fn config(&self) -> &ConnectConfig {
        &self.config
    }
    pub fn last_report(&self) -> &DiscoveryReport {
        &self.last_report
    }
    pub fn add_endpoint(&mut self, endpoint: &str) -> Result<()> {
        validate_endpoint(endpoint, &self.config.network)?;
        ensure!(
            self.candidates.len() < MAX_CANDIDATES
                || self.candidates.contains(endpoint.trim_end_matches('/')),
            "candidate limit reached"
        );
        self.candidates
            .insert(endpoint.trim_end_matches('/').into());
        Ok(())
    }
    pub fn add_manifest(
        &mut self,
        manifest: &SignedManifest,
        trusted_key: &[u8; 32],
    ) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let endpoints = manifest.verify(trusted_key, &self.config, now)?;
        let mut staged = self.candidates.clone();
        staged.extend(
            endpoints
                .into_iter()
                .map(|s| s.trim_end_matches('/').to_owned()),
        );
        ensure!(staged.len() <= MAX_CANDIDATES, "candidate limit reached");
        self.candidates = staged;
        Ok(())
    }
    /// Native DNS is deliberately a discovery hint. It is not DNSSEC authentication.
    pub async fn discover_dns(&mut self) -> Result<Vec<String>> {
        let resolver = hickory_resolver::TokioResolver::builder_tokio()?.build()?;
        let mut errors = Vec::new();
        for seed in self.config.dns_seeds.clone() {
            let result = tokio::time::timeout(
                Duration::from_millis(self.config.timeout_ms),
                resolver.txt_lookup(seed.clone()),
            )
            .await;
            match result {
                Ok(Ok(records)) => {
                    for record in records.answers().iter().take(MAX_CANDIDATES) {
                        let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data else {
                            continue;
                        };
                        let bytes: Vec<u8> = txt
                            .txt_data
                            .iter()
                            .flat_map(|chunk| chunk.iter().copied())
                            .take(2_080)
                            .collect();
                        if let Ok(text) = String::from_utf8(bytes)
                            && let Some(endpoint) = text.strip_prefix("genoryn-rpc-v1=")
                            && let Err(error) = self.add_endpoint(endpoint)
                        {
                            errors.push(format!("{seed}: {error}"));
                        }
                    }
                }
                _ => errors.push(format!("{seed}: DNS lookup unavailable")),
            }
        }
        Ok(errors)
    }
    /// Parallel bounded probes followed by a conservative median-height filter.
    pub async fn refresh(&mut self) -> Result<&DiscoveryReport> {
        ensure!(
            !self.candidates.is_empty(),
            "no configured or discovered RPC endpoints"
        );
        let mut tasks = JoinSet::new();
        for endpoint in &self.candidates {
            let http = self.http.clone();
            let cfg = self.config.clone();
            let endpoint = endpoint.clone();
            tasks.spawn(async move {
                let result = probe(&http, &cfg, &endpoint).await;
                (endpoint, result)
            });
        }
        let mut report = DiscoveryReport::default();
        while let Some(joined) = tasks.join_next().await {
            let (endpoint, result) = joined.context("health-check worker failed")?;
            match result {
                Ok(node) => report.healthy.push(node),
                Err(error) => report.rejected.push(RejectedNode {
                    endpoint,
                    reason: error.to_string(),
                }),
            }
        }
        filter_heights(&mut report, self.config.max_height_skew);
        report.healthy.sort_by_key(|n| {
            (
                n.latency_ms,
                std::cmp::Reverse(n.info.height),
                n.endpoint.clone(),
            )
        });
        self.last_report = report;
        Ok(&self.last_report)
    }
    pub async fn select(&mut self) -> Result<HealthyNode> {
        self.refresh().await?;
        self.last_report
            .healthy
            .first()
            .cloned()
            .context("no healthy synchronized RPC endpoint; inspect last_report")
    }
    /// Read-only API with automatic failover. Mutating/admin routes are intentionally absent.
    pub async fn get_json<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        ensure!(
            path == "/v1/info"
                || path == "/v1/supply"
                || path == "/v1/blocks"
                || valid_blocks_query(path),
            "route is not an allowed read-only endpoint"
        );
        self.refresh().await?;
        let mut failures = Vec::new();
        for node in self.last_report.healthy.clone() {
            let result = async {
                probe(&self.http, &self.config, &node.endpoint).await?;
                let bytes =
                    bounded_get(&self.http, &format!("{}{path}", node.endpoint), MAX_BODY).await?;
                Ok::<T, anyhow::Error>(serde_json::from_slice(&bytes)?)
            }
            .await;
            match result {
                Ok(value) => return Ok(value),
                Err(e) => failures.push(format!("{}: {}", node.endpoint, e)),
            }
        }
        bail!("all healthy endpoints failed: {}", failures.join("; "))
    }
}
fn valid_blocks_query(path: &str) -> bool {
    let Some(query) = path.strip_prefix("/v1/blocks?") else {
        return false;
    };
    let parts: Vec<_> = query.split('&').collect();
    if parts.is_empty() || parts.len() > 2 {
        return false;
    }
    let mut seen = BTreeSet::new();
    parts.iter().all(|part| {
        part.split_once('=').is_some_and(|(key, value)| {
            if !seen.insert(key) || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            match (key, value.parse::<u64>()) {
                ("from", Ok(_)) => true,
                ("limit", Ok(limit)) => (1..=100).contains(&limit),
                _ => false,
            }
        })
    })
}
async fn probe(http: &Client, cfg: &ConnectConfig, endpoint: &str) -> Result<HealthyNode> {
    let url = validate_endpoint(endpoint, &cfg.network)?;
    let start = Instant::now();
    let bytes = bounded_get(
        http,
        &format!("{}/v1/info", endpoint.trim_end_matches('/')),
        64 * 1024,
    )
    .await?;
    let info: NodeInfo = serde_json::from_slice(&bytes).context("invalid node-info response")?;
    ensure!(
        info.network == cfg.network
            && info.chain_id == cfg.chain_id
            && info.genesis_hash == cfg.genesis_hash,
        "node chain identity mismatch"
    );
    ensure!(info.version == 1, "unsupported RPC protocol version");
    ensure!(
        info.public_rpc || is_loopback(&url),
        "remote node has not enabled public RPC"
    );
    ensure!(
        info.best_hash.len() == 64 && hex::decode(&info.best_hash).is_ok(),
        "invalid best block hash"
    );
    Ok(HealthyNode {
        endpoint: endpoint.into(),
        latency_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        info,
    })
}
async fn bounded_get(http: &Client, url: &str, max: usize) -> Result<Vec<u8>> {
    let mut response = http
        .get(url)
        .send()
        .await
        .context("RPC connection failed")?
        .error_for_status()
        .context("RPC returned an HTTP error")?;
    ensure!(
        response
            .content_length()
            .is_none_or(|len| len <= max as u64),
        "RPC response too large"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len().saturating_add(chunk.len()) <= max,
            "RPC response too large"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn filter_heights(report: &mut DiscoveryReport, skew: u64) {
    if report.healthy.is_empty() {
        return;
    }
    let mut heights: Vec<_> = report.healthy.iter().map(|n| n.info.height).collect();
    heights.sort_unstable();
    let median = heights[(heights.len() - 1) / 2];
    let mut accepted = Vec::new();
    for node in report.healthy.drain(..) {
        if node.info.height.abs_diff(median) > skew {
            report.rejected.push(RejectedNode {
                endpoint: node.endpoint,
                reason: format!(
                    "height {} is outside median {} ± {}",
                    node.info.height, median, skew
                ),
            });
        } else {
            accepted.push(node);
        }
    }
    report.healthy = accepted;
}

#[cfg(test)]
mod tests;
