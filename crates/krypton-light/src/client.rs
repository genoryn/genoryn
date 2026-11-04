//! HTTP client for the public node API (`/v1/...`), usable by wallets, explorers,
//! exchanges and the light client. Every response that matters is verified locally.

use crate::{CompactBlock, FlyProof, decode_compact_list};
use anyhow::{Context, Result, bail};
use krypton_core::tree::Frontier;
use krypton_core::types::{BlockHeader, Transaction};
use krypton_core::{Hash32, mmr::MmrProof};
use krypton_crypto::field::{Digest, digest_from_bytes};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone)]
pub struct NodeClient {
    pub base: String,
    http: reqwest::blocking::Client,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct NodeInfo {
    pub network: String,
    pub chain_id: String,
    pub version: String,
    pub height: u64,
    pub tip: String,
    pub genesis: String,
    pub difficulty: String,
    pub supply: u64,
    pub peers: usize,
    pub mempool: usize,
    #[serde(default)]
    pub hashrate: f64,
    #[serde(default)]
    pub reward_pool: u64,
    #[serde(default)]
    pub next_subsidy: u64,
    /// Network summary (estimated miners, GPU equivalents, supply), informational.
    #[serde(default)]
    pub stats: Option<serde_json::Value>,
}

/// Unpaid mining rewards of one miner (informational: payouts themselves are
/// verified against block headers when they arrive).
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct MinerAccount {
    pub unpaid: u64,
    pub next_payout_height: Option<u64>,
    pub payout_interval: u64,
    pub shares_recent: u64,
    pub shares_recent_blocks: u64,
    pub shares_pending: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct WitnessResp {
    path: Vec<String>,
    root: String,
}

fn digest_hex(s: &str) -> Result<Digest> {
    let b: [u8; 32] = hex::decode(s)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("length"))?;
    Ok(digest_from_bytes(&b)?)
}

/// A node's fee suggestions, in gori per KB of transaction.
#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize)]
pub struct FeeRates {
    pub min_per_kb: u64,
    pub economy_per_kb: u64,
    pub normal_per_kb: u64,
    pub fast_per_kb: u64,
}

/// The largest response a node may send (compact-block batches are capped at 16 MiB):
/// a hostile node cannot exhaust the wallet's memory.
const MAX_RESPONSE: u64 = 64 << 20;

fn read_limited(r: reqwest::blocking::Response, max: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    if let Some(len) = r.content_length() {
        anyhow::ensure!(len <= max, "response too large");
    }
    let mut b = Vec::new();
    r.take(max + 1).read_to_end(&mut b)?;
    anyhow::ensure!(b.len() as u64 <= max, "response too large");
    Ok(b)
}

/// A response body read in full (within [`MAX_RESPONSE`]).
pub struct Body(Vec<u8>);
impl Body {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.0)?)
    }
    pub fn bytes(self) -> Result<Vec<u8>> {
        Ok(self.0)
    }
}

impl NodeClient {
    pub fn new(base: &str) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("krypton-light/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            http,
        })
    }

    fn get(&self, path: &str) -> Result<Body> {
        let r = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .with_context(|| format!("GET {path}"))?;
        if !r.status().is_success() {
            let status = r.status();
            let body = read_limited(r, 4096).unwrap_or_default();
            // Nodes answer {"error": "..."}: show that sentence, not the raw response.
            let msg = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(200).collect());
            bail!("{msg} (HTTP {}, {path})", status.as_u16());
        }
        Ok(Body(read_limited(r, MAX_RESPONSE)?))
    }

    pub fn info(&self) -> Result<NodeInfo> {
        self.get("/v1/info")?.json()
    }

    /// Chart series for `range` (1h, 6h, 24h, 7d, 30d, all); informational.
    pub fn stats(&self, range: &str) -> Result<serde_json::Value> {
        self.get(&format!("/v1/stats?range={range}"))?.json()
    }

    /// Height of a confirmed transaction (None while pending or unknown).
    pub fn tx_height(&self, txid: &Hash32) -> Result<Option<u64>> {
        let r = self
            .http
            .get(format!("{}/v1/tx/{}", self.base, hex::encode(txid)))
            .send()
            .context("GET /v1/tx")?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !r.status().is_success() {
            bail!("{} /v1/tx", r.status());
        }
        let v: serde_json::Value = r.json()?;
        Ok(v["height"].as_u64().filter(|_| v["status"] == "confirmed"))
    }

    pub fn raw_block(&self, height: u64) -> Result<krypton_core::types::Block> {
        let b = self.get(&format!("/v1/rawblock/{height}"))?.bytes()?;
        Ok(krypton_core::types::Block::from_bytes(&b)?)
    }

    pub fn miner_account(&self, owner: &Hash32) -> Result<MinerAccount> {
        self
            .get(&format!("/v1/miner/{}", hex::encode(owner)))?
            .json()
    }

    pub fn header(&self, height: u64) -> Result<BlockHeader> {
        self.get(&format!("/v1/header/{height}"))?.json()
    }

    pub fn compact_blocks(&self, from: u64, count: u32) -> Result<Vec<CompactBlock>> {
        let b = self
            .get(&format!("/v1/compact?from={from}&count={count}"))?
            .bytes()?;
        decode_compact_list(&b)
    }

    pub fn frontier(&self, height: u64) -> Result<Frontier> {
        let b = self.get(&format!("/v1/frontier/{height}"))?.bytes()?;
        Frontier::from_bytes(&b).context("bad frontier encoding")
    }

    /// Path for a single position (reveals the position to the server; prefer [`Self::subtree_witness`]).
    pub fn witness(&self, pos: u64) -> Result<(Vec<Digest>, Digest)> {
        let w: WitnessResp = self.get(&format!("/v1/witness/{pos}"))?.json()?;
        Ok((
            w.path
                .iter()
                .map(|s| digest_hex(s))
                .collect::<Result<_>>()?,
            digest_hex(&w.root)?,
        ))
    }

    /// Private witness: downloads the whole 4096-leaf chunk around `pos` plus the
    /// path above it, and computes the path locally. The server learns only the chunk.
    pub fn subtree_witness(&self, pos: u64) -> Result<(Vec<Digest>, Digest)> {
        const CHUNK_LEVEL: u8 = 12;
        let chunk = pos >> CHUNK_LEVEL;
        let r: SubtreeResp = self.get(&format!("/v1/subtree/{chunk}"))?.json()?;
        let leaves: Vec<Digest> = r
            .leaves
            .iter()
            .map(|s| digest_hex(s))
            .collect::<Result<_>>()?;
        let upper: Vec<Digest> = r
            .upper
            .iter()
            .map(|s| digest_hex(s))
            .collect::<Result<_>>()?;
        let root = digest_hex(&r.root)?;
        let mut t = krypton_core::tree::MemTree::new();
        for l in &leaves {
            t.append(*l);
        }
        let local = pos - (chunk << CHUNK_LEVEL);
        let mut path = Vec::with_capacity(krypton_zk_depth());
        let e = krypton_core::tree::empty_roots();
        for level in 0..CHUNK_LEVEL {
            let idx = (local >> level) ^ 1;
            let sib = if (idx << level) >= leaves.len() as u64 {
                e[level as usize]
            } else {
                krypton_core::tree::node_at(&t, leaves.len() as u64, level, idx)
            };
            path.push(sib);
        }
        path.extend(upper);
        // Verify locally against the reported root before use.
        let leaf = leaves
            .get(local as usize)
            .context("position outside chunk")?;
        let mut cur = *leaf;
        for (k, s) in path.iter().enumerate() {
            cur = if (pos >> k) & 1 == 0 {
                krypton_crypto::rescue::merkle_node(&cur, s)
            } else {
                krypton_crypto::rescue::merkle_node(s, &cur)
            };
        }
        if cur != root {
            bail!("subtree witness does not hash to the reported root");
        }
        Ok((path, root))
    }

    pub fn submit(&self, tx: &Transaction) -> Result<Hash32> {
        let r = self
            .http
            .post(format!("{}/v1/tx", self.base))
            .body(tx.to_bytes())
            .send()?;
        if !r.status().is_success() {
            bail!("transaction rejected: {}", r.text().unwrap_or_default());
        }
        let v: serde_json::Value = r.json()?;
        let id = v["txid"].as_str().context("txid")?;
        hex::decode(id)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("txid"))
    }

    pub fn flyclient(&self, heights: &[u64]) -> Result<FlyProof> {
        let q: Vec<String> = heights.iter().map(|h| h.to_string()).collect();
        self
            .get(&format!("/v1/flyclient?heights={}", q.join(",")))?
            .json()
    }

    /// The node's tip, verified FlyClient-style (sampled proof of work and MMR
    /// commitments back to our own genesis). `known` short-cuts an unchanged tip.
    pub fn verified_tip(
        &self,
        params: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        known: Option<&BlockHeader>,
    ) -> Result<BlockHeader> {
        let genesis = krypton_core::consensus::genesis_block(params).hash();
        let info = self.info()?;
        if let Some(k) = known
            && hex::encode(k.hash()) == info.tip
        {
            return Ok(k.clone());
        }
        if info.height == 0 {
            let g = self.header(0)?;
            anyhow::ensure!(g.hash() == genesis, "node serves another chain");
            return Ok(g);
        }
        let mut heights = crate::sample_heights(info.height, 48, rand::random());
        heights.insert(0, 0);
        if let Some(cp) = params.checkpoint {
            heights.push(cp.height);
        }
        heights.sort_unstable();
        heights.dedup();
        let p = self.flyclient(&heights)?;
        let expected: Vec<u64> = heights.into_iter().filter(|h| *h < p.tip.height).collect();
        crate::verify_flyclient(params, &genesis, pow, &p, &expected)?;
        Ok(p.tip)
    }

    /// Whether `h` is part of the chain committed by the verified `tip`.
    pub fn header_in(&self, h: &BlockHeader, tip: &BlockHeader) -> Result<bool> {
        if h.height >= tip.height {
            return Ok(h.height == tip.height && h.hash() == tip.hash());
        }
        let p = self.mmr_proof(h.height, tip.height)?;
        Ok(p.leaf_index == h.height
            && p.leaves == tip.height
            && krypton_core::mmr::verify(
                &tip.mmr_root,
                &krypton_core::mmr::MmrNode::leaf(&h.hash(), h.difficulty),
                &p,
            ))
    }

    pub fn mmr_proof(&self, leaf: u64, at: u64) -> Result<MmrProof> {
        self
            .get(&format!("/v1/mmrproof?leaf={leaf}&at={at}"))?
            .json()
    }

    pub fn nullifier_spent(&self, nf: &Hash32) -> Result<Option<u64>> {
        let v: serde_json::Value = self
            .get(&format!("/v1/nullifier/{}", hex::encode(nf)))?
            .json()?;
        Ok(v["spent_height"].as_u64())
    }

    /// Resolves a short address and checks that the node's answer hashes to it, so a
    /// node cannot substitute another recipient.
    pub fn resolve_short(&self, short: &str) -> Result<String> {
        let (net, id) = krypton_core::address::Address::parse_short(short)?;
        let v: serde_json::Value = self.get(&format!("/v1/address/{}", short.trim()))?.json()?;
        let full = v["address"].as_str().context("address")?.to_string();
        let a = krypton_core::address::Address::decode(&full)?;
        if a.network != net || a.short_id() != id {
            bail!("node returned an address that does not match the short address");
        }
        Ok(full)
    }

    pub fn publish_address(&self, address: &str) -> Result<String> {
        let r = self
            .http
            .post(format!("{}/v1/address", self.base))
            .body(address.to_string())
            .send()
            .context("node unreachable")?;
        let ok = r.status().is_success();
        let v: serde_json::Value = r.json().unwrap_or_default();
        if !ok {
            bail!("{}", v["error"].as_str().unwrap_or("the node refused the address"));
        }
        Ok(v["short"].as_str().context("short")?.to_string())
    }

    /// Whether this node resolves the short address.
    pub fn resolves(&self, short: &str) -> Result<bool> {
        let r = self
            .http
            .get(format!("{}/v1/address/{}", self.base, short.trim()))
            .send()
            .context("node unreachable")?;
        match r.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            s => bail!("HTTP {s}"),
        }
    }

    /// The latest `count` block summaries, newest first; informational.
    pub fn blocks(&self, count: u32) -> Result<serde_json::Value> {
        self.get(&format!("/v1/blocks?count={}", count.min(100)))?.json()
    }

    /// Fee suggestions per KB from the node's mempool (None for nodes before 1.0.7).
    pub fn fee_rates(&self) -> Result<Option<FeeRates>> {
        let r = self
            .http
            .get(format!("{}/v1/fee", self.base))
            .send()
            .context("node unreachable")?;
        if r.status().as_u16() == 404 {
            return Ok(None);
        }
        anyhow::ensure!(r.status().is_success(), "fee estimate: HTTP {}", r.status());
        Ok(Some(serde_json::from_slice(&read_limited(r, 64 * 1024)?)?))
    }

    /// Peers, mempool and supply summaries for dashboards; informational.
    pub fn peers(&self) -> Result<serde_json::Value> {
        self.get("/v1/peers")?.json()
    }
    pub fn mempool(&self) -> Result<serde_json::Value> {
        self.get("/v1/mempool")?.json()
    }
    pub fn supply(&self) -> Result<serde_json::Value> {
        self.get("/v1/supply")?.json()
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct SubtreeResp {
    pub leaves: Vec<String>,
    pub upper: Vec<String>,
    pub root: String,
}

fn krypton_zk_depth() -> usize {
    40
}
