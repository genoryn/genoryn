use anyhow::{Context, Result, ensure};
use genoryn_core::{block::Block, params::Network};
use reqwest::{Client, Url};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

/// The node answered an administrative request with an error status.
#[derive(Debug)]
pub struct Rejected {
    pub status: u16,
    pub body: String,
}
impl Rejected {
    /// The admin token was refused: the node is not the one whose token was read.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self.status, 401 | 403)
    }
}
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "node rejected request: {}", self.body)
    }
}
impl std::error::Error for Rejected {}

#[derive(Clone)]
pub struct RpcClient {
    http: Client,
    base: Url,
    token: Option<String>,
}
impl RpcClient {
    pub fn new(url: &str, token_file: Option<&Path>) -> Result<Self> {
        let mut base = Url::parse(url)?;
        ensure!(
            base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "RPC URL must not contain credentials/query/fragment"
        );
        ensure!(
            !base.host_str().is_some_and(crate::net::addr::is_onion_name),
            ".onion RPC endpoints need Tor routing, which this client does not provide; refusing to resolve one through system DNS"
        );
        let loopback = base
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        ensure!(
            base.scheme() == "https" || (base.scheme() == "http" && loopback),
            "HTTP RPC is allowed only at literal loopback IPs"
        );
        let token = if let Some(path) = token_file {
            ensure!(loopback, "admin tokens must never be sent to remote nodes");
            Some(std::fs::read_to_string(path)?.trim().to_owned())
        } else {
            None
        };
        base.set_path("/");
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(600))
            .no_proxy()
            .build()?;
        Ok(Self { http, base, token })
    }
    /// A remote node's public RPC, reached only through Tor at `proxy` (a
    /// `socks5h://` URL, so Tor resolves every name). Onion services may use
    /// plain HTTP, which Tor encrypts end to end; clearnet nodes need HTTPS.
    /// Never carries an admin token and never connects directly.
    pub fn through_tor(url: &str, proxy: &str) -> Result<Self> {
        let mut base = Url::parse(url)?;
        ensure!(
            base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "RPC URL must not contain credentials/query/fragment"
        );
        ensure!(
            proxy.starts_with("socks5h://"),
            "light wallet traffic needs a Tor proxy that resolves names"
        );
        let onion = base.host_str().is_some_and(crate::net::addr::is_onion_name);
        ensure!(
            base.scheme() == "https" || (base.scheme() == "http" && onion),
            "a remote node needs HTTPS unless it is an onion service"
        );
        base.set_path("/");
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(600))
            .proxy(reqwest::Proxy::all(proxy)?)
            .build()?;
        Ok(Self {
            http,
            base,
            token: None,
        })
    }
    /// Whether administrative calls (`post`) can be made.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }
    /// Hands a transaction to the node's public relay (no token needed).
    pub async fn relay<T: DeserializeOwned>(&self, tx: &impl Serialize) -> Result<T> {
        let response = self
            .http
            .post(self.base.join("v1/relay")?)
            .json(tx)
            .send()
            .await?;
        decode_answer(response).await
    }
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.http.get(self.base.join(path)?).send().await?;
        ensure!(
            response.content_length().unwrap_or(0) <= 8_000_000,
            "RPC response too large"
        );
        let response = response.error_for_status()?;
        let bytes = bounded_response(response).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let token = self
            .token
            .as_ref()
            .context("administrative token file is required for this operation")?;
        let response = self
            .http
            .post(self.base.join(path)?)
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        decode_answer(response).await
    }
    pub async fn chain(&self, network: Network) -> Result<Vec<Block>> {
        self.chain_from(network, Vec::new()).await
    }
    /// The node's chain, reusing `known` (blocks 1..=n this computer already
    /// holds) up to the last one the node still has at the same height and
    /// hash, so only new blocks are downloaded.
    pub async fn chain_from(&self, network: Network, mut known: Vec<Block>) -> Result<Vec<Block>> {
        let info: serde_json::Value = self.get("v1/info").await?;
        ensure!(
            info["chain_id"] == network.chain_id()
                && info["genesis_hash"] == hex::encode(network.genesis()),
            "RPC network/genesis mismatch"
        );
        let tip = info["best_hash"]
            .as_str()
            .context("RPC tip missing")?
            .to_owned();
        let height = info["height"].as_u64().context("RPC height missing")?;
        ensure!(
            height <= 1_000_000,
            "wallet full-rescan size exceeds configured limit"
        );
        known.truncate(height as usize);
        // Walk back from the newest known block to one the node still has.
        while let Some(last) = known.last() {
            let theirs = self.block_page(last.header.height, 1).await?;
            if theirs[0].hash()? == last.hash()? {
                break;
            }
            known.pop();
        }
        let mut blocks = known;
        if (blocks.len() as u64) < height {
            let more = self.blocks_range(blocks.len() as u64 + 1, height).await?;
            blocks.extend(more);
        }
        ensure!(
            blocks
                .last()
                .map(|b| b.hash().map(hex::encode))
                .transpose()?
                .unwrap_or_else(|| hex::encode(network.genesis()))
                == tip,
            "chain changed during scan; retry sync"
        );
        Ok(blocks)
    }
    /// Up to `limit` consecutive blocks starting at `from`. Nodes before 1.1
    /// return one block per request; any nonempty consecutive page is valid.
    pub async fn block_page(&self, from: u64, limit: usize) -> Result<Vec<Block>> {
        let limit = limit.clamp(1, MAX_PAGE);
        let page: Vec<Block> = self
            .get(&format!("v1/blocks?from={from}&limit={limit}"))
            .await?;
        ensure!(!page.is_empty(), "node returned no block at height {from}");
        ensure!(page.len() <= limit, "node exceeded requested page size");
        for (offset, block) in page.iter().enumerate() {
            ensure!(
                block.header.height == from + offset as u64,
                "node returned the wrong block for height {}",
                from + offset as u64
            );
        }
        Ok(page)
    }
    /// Blocks `from..=to`, in pages.
    pub async fn blocks_range(&self, from: u64, to: u64) -> Result<Vec<Block>> {
        let mut blocks = Vec::with_capacity(to.saturating_sub(from) as usize + 1);
        let mut next = from;
        while next <= to {
            let wanted = (to - next + 1).min(MAX_PAGE as u64) as usize;
            let page = self.block_page(next, wanted).await?;
            next += page.len() as u64;
            blocks.extend(page);
        }
        Ok(blocks)
    }
}

/// Blocks requested per page; nodes since 1.1 serve up to 32.
pub const MAX_PAGE: usize = 32;

async fn decode_answer<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = bounded_response(response).await?;
    let text = String::from_utf8(bytes)?;
    if !status.is_success() {
        return Err(Rejected {
            status: status.as_u16(),
            body: text.chars().take(1000).collect(),
        }
        .into());
    }
    Ok(serde_json::from_str(&text)?)
}
async fn bounded_response(mut response: reqwest::Response) -> Result<Vec<u8>> {
    const LIMIT: usize = 12_000_000;
    ensure!(
        response.content_length().is_none_or(|n| n <= LIMIT as u64),
        "RPC response too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= LIMIT.saturating_sub(bytes.len()),
            "RPC response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_endpoints_refuse_onion_names_and_remote_plain_http() {
        assert!(RpcClient::new("https://abc.onion", None).is_err());
        assert!(RpcClient::new("http://abc.onion", None).is_err());
        assert!(RpcClient::new("http://203.0.113.1:29611", None).is_err());
        assert!(RpcClient::new("http://127.0.0.1:29611", None).is_ok());
    }
    #[test]
    fn tor_routed_clients_accept_onions_and_refuse_plain_clearnet() {
        let onion = crate::net::addr::tests::test_onion(4).hostname();
        let proxy = "socks5h://genoryn:secret@127.0.0.1:9050";
        assert!(RpcClient::through_tor(&format!("http://{onion}:29612"), proxy).is_ok());
        assert!(RpcClient::through_tor("https://rpc.example.org", proxy).is_ok());
        assert!(RpcClient::through_tor("http://rpc.example.org", proxy).is_err());
        assert!(RpcClient::through_tor("http://203.0.113.1:29612", proxy).is_err());
        assert!(
            RpcClient::through_tor("https://rpc.example.org", "socks5://127.0.0.1:9050").is_err(),
            "names must be resolved by Tor"
        );
        assert!(RpcClient::through_tor("https://user:pw@rpc.example.org", proxy).is_err());
    }
}
