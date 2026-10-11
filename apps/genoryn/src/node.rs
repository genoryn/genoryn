use crate::{
    net::{Link, NetClass, NetContext, NetworkOptions, PeerAddr},
    relay::{RelayScope, RelayState},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use genoryn_core::{
    block::Block,
    chain::{Acceptance, Chain, unix_time},
    codec::{self, Hash},
    names,
    params::Network,
    privacy::ShieldedTx,
};
use genoryn_wallet_core::{
    keys::decode_address, registry_store::RegistryStore, username::ReceiverBundle,
};
use rand08::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicU64},
};
use tokio::sync::{Notify, Semaphore, broadcast};

/// Shown with every network-privacy status so the two kinds of privacy are not confused.
const LOCAL_TRANSACTIONS: &str = "local-transactions.json";
pub const PRIVACY_LAYERS: &str = "Network privacy (Tor) hides this computer's IP address from peers. On-chain privacy (Orchard/Halo 2 shielded notes) hides amounts and receiving addresses. Neither replaces the other, and neither makes anyone 100% anonymous.";

pub struct NodeData {
    pub chain: Chain,
    pub registry: RegistryStore,
}
#[derive(Clone)]
pub struct Node {
    pub data: Arc<Mutex<NodeData>>,
    pub network: Network,
    pub data_dir: PathBuf,
    pub token: Arc<String>,
    /// Clearnet handshake identity, random for each run (see `NetContext::tor_id`).
    pub id: Hash,
    /// Clearnet listener, or for nodes without one the address excluded from discovery.
    pub p2p_listen: SocketAddr,
    pub peers: Arc<Mutex<BTreeMap<Hash, Peer>>>,
    pub known: Arc<Mutex<BTreeSet<PeerAddr>>>,
    pub events: broadcast::Sender<Event>,
    pub public_rpc: bool,
    pub work_gate: Arc<Semaphore>,
    pub rpc_gate: Arc<Semaphore>,
    /// Separate from `rpc_gate` so public load cannot starve the loopback admin RPC.
    pub public_rpc_gate: Arc<Semaphore>,
    pub last_peer_rotation: Arc<Mutex<Option<std::time::Instant>>>,
    pub limits: crate::config::ResourceLimits,
    pub net: Arc<NetContext>,
    pub relay: Arc<RelayState>,
    pub next_connection: Arc<AtomicU64>,
    /// Woken by the administrative reconnect request to retry dialing at once.
    pub reconnect: Arc<Notify>,
}
#[derive(Clone, Serialize)]
pub struct Peer {
    pub address: PeerAddr,
    pub inbound: bool,
    pub height: u64,
    pub link: Link,
    pub network: NetClass,
    #[serde(skip)]
    pub connection: u64,
}
#[derive(Clone)]
pub enum Event {
    Block(Box<Block>),
    /// Flood relay; never sent back to connection `from`.
    Transaction {
        tx: ShieldedTx,
        from: Option<u64>,
        scope: RelayScope,
    },
    /// A transaction created here, for exactly one connection.
    Stem {
        tx: ShieldedTx,
        to: u64,
    },
}
/// Block acceptance plus the local tip context the P2P layer needs for policy.
pub struct BlockOutcome {
    pub acceptance: Acceptance,
    pub tip_work: u128,
    pub next_difficulty: u64,
}

impl Node {
    pub fn open(
        network: Network,
        data_dir: PathBuf,
        p2p_listen: SocketAddr,
        public_rpc: bool,
    ) -> Result<Self> {
        Self::open_with_limits(
            network,
            data_dir,
            p2p_listen,
            public_rpc,
            crate::config::ResourceLimits::default(),
        )
    }
    pub fn open_with_limits(
        network: Network,
        data_dir: PathBuf,
        p2p_listen: SocketAddr,
        public_rpc: bool,
        limits: crate::config::ResourceLimits,
    ) -> Result<Self> {
        Self::open_with_options(
            network,
            data_dir,
            p2p_listen,
            public_rpc,
            limits,
            NetworkOptions::direct(p2p_listen),
        )
    }
    pub fn open_with_options(
        network: Network,
        data_dir: PathBuf,
        p2p_listen: SocketAddr,
        public_rpc: bool,
        limits: crate::config::ResourceLimits,
        options: NetworkOptions,
    ) -> Result<Self> {
        limits.validate()?;
        network.ensure_runnable()?;
        std::fs::create_dir_all(&data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let net = NetContext::new(options, &data_dir)?;
        let token = read_or_create_secret(&data_dir.join("admin.token"))?;
        // A persistent handshake identity would let peers recognize this node across
        // restarts, addresses and transports, so a fresh one is drawn every run.
        let mut id = [0u8; 32];
        rand08::rngs::OsRng.fill_bytes(&mut id);
        let mut chain = Chain::open(&data_dir.join("chain.sqlite"), network)?;
        chain.configure_mempool(limits.mempool_transactions, limits.mempool_bytes)?;
        let registry = RegistryStore::open(
            &data_dir.join("usernames.sqlite"),
            network,
            Some(&data_dir.join("usernames.json")),
        )?;
        let known = load_peer_cache(&data_dir.join(net.policy.cache_file()), &net)?;
        let (events, _) = broadcast::channel(64);
        Ok(Self {
            data: Arc::new(Mutex::new(NodeData { chain, registry })),
            network,
            data_dir,
            token: Arc::new(token),
            id,
            p2p_listen,
            peers: Arc::new(Mutex::new(BTreeMap::new())),
            last_peer_rotation: Arc::new(Mutex::new(None)),
            known: Arc::new(Mutex::new(known)),
            events,
            public_rpc,
            work_gate: Arc::new(Semaphore::new(1)),
            rpc_gate: Arc::new(Semaphore::new(limits.rpc_concurrency)),
            public_rpc_gate: Arc::new(Semaphore::new(limits.rpc_concurrency)),
            limits,
            net: Arc::new(net),
            relay: Arc::new(RelayState::default()),
            next_connection: Arc::new(AtomicU64::new(1)),
            reconnect: Arc::new(Notify::new()),
        })
    }
    /// Network privacy status for the loopback administrative RPC only: it
    /// includes this node's onion address and peer transport mix.
    pub fn network_status(&self) -> Result<Value> {
        let peers = self
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("peer state poisoned"))?;
        let count = |link: Link, inbound: bool| {
            peers
                .values()
                .filter(|peer| peer.link == link && peer.inbound == inbound)
                .count()
        };
        let connections = json!({
            "total": peers.len(),
            "clearnet_inbound": count(Link::Clearnet, true),
            "clearnet_outbound": count(Link::Clearnet, false),
            "tor_outbound": count(Link::Tor, false),
            "onion_inbound": count(Link::OnionInbound, true),
        });
        drop(peers);
        let known = self
            .known
            .lock()
            .map_err(|_| anyhow::anyhow!("peer state poisoned"))?;
        let onion_known = known.iter().filter(|address| address.is_onion()).count();
        let known =
            json!({"total": known.len(), "onion": onion_known, "ip": known.len() - onion_known});
        let mode = self.net.options.mode;
        let mut warnings = Vec::new();
        if mode != crate::net::NetworkMode::TorOnly {
            warnings.push(mode.privacy_notice());
        }
        let tor = self.net.tor.as_ref().map(|tor| tor.snapshot());
        if let Some(status) = &tor
            && !status.ready
        {
            warnings.push(if mode == crate::net::NetworkMode::TorOnly {
                "Tor is unavailable: peer connections and transaction broadcasts are paused until it recovers. Nothing falls back to direct connections."
            } else {
                "Tor is unavailable: onion peers cannot be reached until it recovers."
            });
        }
        if mode == crate::net::NetworkMode::TorOnly
            && tor.as_ref().is_some_and(|status| status.ready)
            && connections["total"] == 0
            && self.net.connector.counters.snapshot().tor_failures > 0
        {
            warnings.push(
                "Tor works, but no peer could be reached through it yet. Bootstrap peers need an onion address or must accept connections from Tor exits; add an onion peer or wait for one to be published.",
            );
        }
        Ok(json!({
            "mode": mode.name(),
            "mode_notice": mode.privacy_notice(),
            "privacy_layers": PRIVACY_LAYERS,
            "warnings": warnings,
            "clearnet_listener": self.net.options.listen.map(|listen| listen.to_string()),
            "onion_target": self.net.onion_target().map(|target| target.to_string()),
            "tor": tor,
            "connections": connections,
            "known_addresses": known,
            "transport": self.net.connector.counters.snapshot(),
            "local_broadcasts_pending": self.relay.pending(),
            "anonymity_claim": "none: Tor reduces IP exposure; it does not guarantee anonymity",
        }))
    }
    pub fn info(&self) -> Result<Value> {
        let d = self
            .data
            .lock()
            .map_err(|_| anyhow::anyhow!("node state poisoned"))?;
        let s = &d.chain.state;
        let peers = self
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("peer state poisoned"))?;
        let peer_count = peers.len();
        let inbound_peers = peers.values().filter(|peer| peer.inbound).count();
        let outbound_peers = peer_count - inbound_peers;
        drop(peers);
        let database_bytes = database_size(&self.data_dir).ok();
        Ok(
            json!({"network":self.network.name(),"chain_id":self.network.chain_id(),"genesis_hash":hex::encode(self.network.genesis()),"version":1,"height":s.height,"best_hash":hex::encode(s.best_hash),"difficulty":s.next_difficulty().to_string(),"estimated_hashrate_hs":s.next_difficulty() as f64/60.0,"hashrate_estimate_basis":"next difficulty / 60-second target; not observed miner telemetry","peer_count":peer_count,"inbound_peers":inbound_peers,"outbound_peers":outbound_peers,"database_bytes":database_bytes,"mempool_count":d.chain.mempool.len(),"issued_supply":genoryn_core::emission::issued(s.height).to_string(),"registered_usernames":d.registry.len()?,"max_registered_usernames":genoryn_wallet_core::registry_store::MAX_REGISTERED_IDENTITIES,"public_rpc":self.public_rpc,"mainnet_finalized":true,"treasury":if self.network==genoryn_core::params::Network::Mainnet{"burned: paid to a provably unspendable address"}else{"public test key"},"privacy_status":"experimental; independent audit required"}),
        )
    }
    pub async fn accept_block(&self, block: Block) -> Result<Value> {
        let accepted = self.accept_block_outcome(block).await?.acceptance;
        Ok(
            json!({"hash":hex::encode(accepted.hash),"became_best":accepted.became_best,"reorg_depth":accepted.reorg_depth}),
        )
    }
    pub async fn accept_block_outcome(&self, block: Block) -> Result<BlockOutcome> {
        let permit = self.work_gate.clone().acquire_owned().await?;
        let data = self.data.clone();
        let clone = block.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut data = data.lock().map_err(|_| anyhow::anyhow!("state poisoned"))?;
            let acceptance = data.chain.accept(clone)?;
            Ok::<_, anyhow::Error>(BlockOutcome {
                acceptance,
                tip_work: data.chain.state.work,
                next_difficulty: data.chain.state.next_difficulty(),
            })
        })
        .await??;
        let accepted = &outcome.acceptance;
        if accepted.became_best {
            let _ = self.events.send(Event::Block(Box::new(block)));
            tracing::info!(
                height = self
                    .data
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .chain
                    .state
                    .height,
                reorg_depth = accepted.reorg_depth,
                "accepted block"
            );
        }
        Ok(outcome)
    }
    async fn admit_to_mempool(&self, tx: ShieldedTx) -> Result<Hash> {
        let permit = self.work_gate.clone().acquire_owned().await?;
        let data = self.data.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            data.lock()
                .map_err(|_| anyhow::anyhow!("state poisoned"))?
                .chain
                .submit(tx)
        })
        .await?
    }
    /// A transaction created by this node's own wallet (RPC). It is first sent to
    /// a single relay peer; see `relay` for the broadcast policy.
    pub async fn submit_tx(&self, tx: ShieldedTx) -> Result<Value> {
        let id = self.admit_to_mempool(tx.clone()).await?;
        if self.relay.register_local(id) {
            self.persist_local();
            tokio::spawn(crate::p2p::broadcast_local(self.clone(), id, tx));
        } else {
            let _ = self.events.send(Event::Transaction {
                tx,
                from: None,
                scope: crate::p2p::local_fluff_scope(self),
            });
        }
        Ok(json!({"txid":hex::encode(id),"status":"accepted to mempool; unconfirmed"}))
    }
    /// Saves this node's own pending transactions so a restart cannot lose them.
    /// Only already-shielded transaction bytes are stored, and only while pending.
    fn persist_local(&self) {
        let path = self.data_dir.join(LOCAL_TRANSACTIONS);
        let result = (|| -> Result<()> {
            let ids = self.relay.local_ids();
            let encoded = {
                let d = self
                    .data
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?;
                ids.iter()
                    .filter_map(|id| d.chain.mempool.get(id))
                    .map(|tx| codec::encode(tx).map(hex::encode))
                    .collect::<Result<Vec<_>>>()?
            };
            if encoded.is_empty() {
                if path.exists() {
                    std::fs::remove_file(&path)?;
                }
                return Ok(());
            }
            let temporary = path.with_extension("tmp");
            std::fs::write(&temporary, serde_json::to_vec(&encoded)?)?;
            std::fs::rename(&temporary, &path)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(error = %error, "could not save pending local transactions");
        }
    }
    /// A local transaction left the mempool: mined, expired, conflicted or evicted.
    pub fn forget_local(&self, id: &Hash) {
        self.relay.forget(id);
        self.persist_local();
    }
    /// Re-admits own transactions saved before a restart. Each is validated
    /// against the current chain again; mined, expired, conflicting or otherwise
    /// invalid ones are dropped. Accepted ones restart their broadcast policy.
    pub async fn restore_local(&self) {
        let path = self.data_dir.join(LOCAL_TRANSACTIONS);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let limit = crate::relay::MAX_LOCAL * (2 * codec::MAX_TX_BYTES + 4) + 2;
        let entries = match serde_json::from_slice::<Vec<String>>(&bytes) {
            Ok(entries) if bytes.len() <= limit && entries.len() <= crate::relay::MAX_LOCAL => {
                entries
            }
            _ => {
                tracing::warn!("ignoring an unreadable pending local transaction file");
                Vec::new()
            }
        };
        let mut restored = 0usize;
        for entry in entries {
            let Ok(tx) = hex::decode(&entry)
                .map_err(anyhow::Error::from)
                .and_then(|raw| codec::decode::<ShieldedTx>(&raw, codec::MAX_TX_BYTES))
            else {
                continue;
            };
            match self.admit_to_mempool(tx.clone()).await {
                Ok(id) if self.relay.register_local(id) => {
                    restored += 1;
                    tokio::spawn(crate::p2p::broadcast_local(self.clone(), id, tx));
                }
                _ => {}
            }
        }
        self.persist_local();
        if restored > 0 {
            tracing::info!(restored, "pending local transactions restored");
        }
    }
    /// A transaction a light wallet handed to this node through public RPC.
    /// Validated exactly like one from a peer and relayed the same way: it is
    /// not this node's own, so it gets no stem and is not kept on disk.
    pub async fn relay_remote(&self, tx: ShieldedTx) -> Result<Hash> {
        let id = self.admit_to_mempool(tx.clone()).await?;
        let _ = self.events.send(Event::Transaction {
            tx,
            from: None,
            scope: RelayScope::All,
        });
        Ok(id)
    }
    /// A transaction received from peer connection `from`, relayed onward.
    pub async fn relay_tx(&self, tx: ShieldedTx, from: u64) -> Result<Hash> {
        let id = self.admit_to_mempool(tx.clone()).await?;
        let _ = self.events.send(Event::Transaction {
            tx,
            from: Some(from),
            scope: RelayScope::All,
        });
        Ok(id)
    }
}
/// Loads this mode's discovery cache, keeping only addresses the mode can reach.
fn load_peer_cache(path: &Path, net: &NetContext) -> Result<BTreeSet<PeerAddr>> {
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let bytes = std::fs::read(path)?;
    ensure!(bytes.len() <= 65_536, "peer cache too large");
    // Entries are parsed one by one so a single unreadable address cannot discard the cache.
    let Ok(entries) = serde_json::from_slice::<Vec<String>>(&bytes) else {
        tracing::warn!(
            "discarding malformed discovery cache; chain database remains independently validated"
        );
        return Ok(BTreeSet::new());
    };
    Ok(entries
        .iter()
        .filter_map(|entry| entry.parse::<PeerAddr>().ok())
        .filter(|address| net.policy.reachable(address))
        .collect())
}
/// Bounded, nonrecursive accounting of the two databases and their WAL/SHM files.
/// Wallets, credentials, logs and unrelated files are never inspected.
fn database_size(directory: &Path) -> Result<u64> {
    let mut total = 0u64;
    for name in [
        "chain.sqlite",
        "chain.sqlite-wal",
        "chain.sqlite-shm",
        "usernames.sqlite",
        "usernames.sqlite-wal",
        "usernames.sqlite-shm",
    ] {
        match std::fs::symlink_metadata(directory.join(name)) {
            Ok(metadata) => {
                ensure!(metadata.is_file(), "database size unavailable");
                total = total
                    .checked_add(metadata.len())
                    .context("database size overflow")?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(total)
}

fn read_or_create_secret(path: &Path) -> Result<String> {
    if path.exists() {
        let s = std::fs::read_to_string(path)?;
        let s = s.trim();
        ensure!(
            s.len() == 64 && hex::decode(s).is_ok(),
            "invalid local node key/token file"
        );
        return Ok(s.to_owned());
    }
    let mut bytes = [0u8; 32];
    rand08::rngs::OsRng.fill_bytes(&mut bytes);
    let text = hex::encode(bytes);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(text)
}
type ApiResult = std::result::Result<Json<Value>, (StatusCode, Json<Value>)>;
fn fail(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":error.to_string()})),
    )
}
fn authorize(
    node: &Node,
    headers: &HeaderMap,
) -> std::result::Result<(), (StatusCode, Json<Value>)> {
    check_token(node, headers).map_err(|error| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":error.to_string()})),
        )
    })
}
fn check_token(node: &Node, headers: &HeaderMap) -> Result<()> {
    let supplied = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .context("authentication required")?;
    // Constant-time byte comparison for equal-length local capability tokens.
    ensure!(
        supplied.len() == node.token.len()
            && supplied
                .bytes()
                .zip(node.token.bytes())
                .fold(0u8, |x, (a, b)| x | (a ^ b))
                == 0,
        "invalid authentication"
    );
    Ok(())
}
async fn info(State(node): State<Node>) -> ApiResult {
    node.info().map(Json).map_err(fail)
}
async fn genesis(State(node): State<Node>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(node.network.genesis_block()).map_err(fail)?,
    ))
}
async fn supply(State(node): State<Node>) -> ApiResult {
    let height = node.data.lock().map_err(fail)?.chain.state.height;
    Ok(Json(
        serde_json::to_value(genoryn_core::emission::supply(height)).map_err(fail)?,
    ))
}
#[derive(Deserialize)]
struct Page {
    #[serde(default = "one")]
    from: u64,
    #[serde(default = "eight")]
    limit: usize,
}
fn one() -> u64 {
    1
}
fn eight() -> usize {
    8
}
/// Blocks one `v1/blocks` answer may hold. Nodes before 1.1 answer one block
/// per request; clients accept any nonempty consecutive page.
pub const MAX_BLOCK_PAGE: usize = 32;
/// Encoded JSON a page may reach before it ends (always at least one block),
/// below the 8 MB clients accept.
const MAX_PAGE_JSON_BYTES: usize = 4_000_000;
async fn blocks(
    State(node): State<Node>,
    Query(page): Query<Page>,
) -> std::result::Result<Json<Vec<Block>>, (StatusCode, Json<Value>)> {
    let selected: Vec<Block> = {
        let d = node.data.lock().map_err(fail)?;
        let from = page.from.max(1).saturating_sub(1) as usize;
        d.chain
            .active
            .iter()
            .skip(from)
            .take(page.limit.clamp(1, MAX_BLOCK_PAGE))
            .cloned()
            .collect()
    };
    let mut bytes = 0usize;
    let mut answer = Vec::with_capacity(selected.len());
    for block in selected {
        let size = serde_json::to_vec(&block).map_err(fail)?.len();
        if !answer.is_empty() && bytes + size > MAX_PAGE_JSON_BYTES {
            break;
        }
        bytes += size;
        answer.push(block);
    }
    Ok(Json(answer))
}
async fn peers(State(node): State<Node>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(
            node.peers
                .lock()
                .map_err(fail)?
                .values()
                .cloned()
                .collect::<Vec<_>>(),
        )
        .map_err(fail)?,
    ))
}
async fn network_status(State(node): State<Node>) -> ApiResult {
    node.network_status().map(Json).map_err(fail)
}
/// SOCKS proxy for the wallet's auxiliary HTTPS in tor-only mode. The URL may
/// carry the built-in bridge's credentials, so the admin token is required.
async fn network_proxy(State(node): State<Node>, headers: HeaderMap) -> ApiResult {
    authorize(&node, &headers)?;
    match node.net.auxiliary_proxy() {
        Some(url) => Ok(Json(json!({"proxy": url.as_str()}))),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"no Tor proxy: not in tor-only mode, or Tor is not running yet"})),
        )),
    }
}
/// Re-checks Tor at once and retries peers now instead of after their backoff.
async fn network_reconnect(State(node): State<Node>, headers: HeaderMap) -> ApiResult {
    authorize(&node, &headers)?;
    if let Some(tor) = &node.net.tor {
        tor.request_reconnect();
    }
    node.reconnect.notify_one();
    Ok(Json(json!({"reconnect_requested": true})))
}
#[derive(Deserialize)]
struct MiningRequest {
    wallet: String,
}
async fn template(
    State(node): State<Node>,
    headers: HeaderMap,
    Json(request): Json<MiningRequest>,
) -> ApiResult {
    authorize(&node, &headers)?;
    let receiver = decode_address(node.network, &request.wallet).map_err(fail)?;
    let permit = node.work_gate.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"proof worker busy; retry"})),
        )
    })?;
    let data = node.data.clone();
    let block = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let plan = data
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .chain
            .template_plan()?;
        plan.build(receiver)
    })
    .await
    .map_err(fail)?
    .map_err(fail)?;
    Ok(Json(serde_json::to_value(block).map_err(fail)?))
}
async fn submit_block(
    State(node): State<Node>,
    headers: HeaderMap,
    Json(block): Json<Block>,
) -> ApiResult {
    authorize(&node, &headers)?;
    node.accept_block(block).await.map(Json).map_err(fail)
}
async fn submit_tx(
    State(node): State<Node>,
    headers: HeaderMap,
    Json(tx): Json<ShieldedTx>,
) -> ApiResult {
    authorize(&node, &headers)?;
    node.submit_tx(tx).await.map(Json).map_err(fail)
}
/// Public relay for light wallets; no token, validated like a peer relay.
async fn relay_tx(State(node): State<Node>, Json(tx): Json<ShieldedTx>) -> ApiResult {
    let id = node.relay_remote(tx).await.map_err(fail)?;
    Ok(Json(json!({"txid": hex::encode(id), "status": "relayed"})))
}
#[derive(Deserialize)]
struct NameQuery {
    username: String,
}
async fn resolve(State(node): State<Node>, Query(query): Query<NameQuery>) -> ApiResult {
    let bundle = node
        .data
        .lock()
        .map_err(fail)?
        .registry
        .resolve(&query.username, node.network, unix_time())
        .map_err(fail)?;
    Ok(Json(serde_json::to_value(bundle).map_err(fail)?))
}
async fn renewal_record(State(node): State<Node>, Query(query): Query<NameQuery>) -> ApiResult {
    let bundle = node
        .data
        .lock()
        .map_err(fail)?
        .registry
        .record_for_renewal(&query.username, node.network, unix_time())
        .map_err(fail)?;
    Ok(Json(serde_json::to_value(bundle).map_err(fail)?))
}
#[derive(Deserialize)]
struct ChainNameQuery {
    name: String,
}
#[derive(Deserialize)]
struct OwnerQuery {
    owner: String,
}
fn name_json(network: Network, entry: &names::NameEntry) -> Value {
    let receiver =
        Option::<orchard::Address>::from(orchard::Address::from_raw_address_bytes(&entry.receiver))
            .and_then(|address| genoryn_wallet_core::keys::encode_address(network, address).ok());
    json!({
        "name": entry.name,
        "receiver": receiver,
        "owner": hex::encode(entry.owner),
        "sequence": entry.sequence,
        "registered_height": entry.registered_height,
        "updated_height": entry.updated_height,
        "txid": hex::encode(entry.txid),
        "burned_atomic": entry.burned.to_string(),
    })
}
/// On-chain username status: its registration, any pending registration in the
/// mempool, and what registering it costs.
async fn chain_name(State(node): State<Node>, Query(query): Query<ChainNameQuery>) -> ApiResult {
    let name = names::canonical_name(&query.name).map_err(fail)?;
    let d = node.data.lock().map_err(fail)?;
    let registered = d.chain.state.names.get(&name);
    let pending = d
        .chain
        .mempool
        .values()
        .flat_map(|tx| names::operations(node.network, tx))
        .any(|operation| operation.record.name == name);
    Ok(Json(json!({
        "name": name,
        "registered": registered.map(|entry| name_json(node.network, entry)),
        "pending": pending,
        "available": registered.is_none() && !pending,
        "price_atomic": names::registration_price(&name).to_string(),
        "update_price_atomic": names::UPDATE_PRICE.to_string(),
        "height": d.chain.state.height,
    })))
}
/// Nullifiers spent by transactions waiting in the mempool. They are public on the
/// P2P network already; wallets use them to avoid double-spending their own pending
/// coins with a second transfer.
async fn mempool_nullifiers(State(node): State<Node>) -> ApiResult {
    let d = node.data.lock().map_err(fail)?;
    let nullifiers: Vec<String> = d
        .chain
        .mempool
        .values()
        .flat_map(|tx| {
            tx.bundle
                .actions
                .iter()
                .map(|action| hex::encode(action.nullifier))
        })
        .collect();
    Ok(Json(json!({ "nullifiers": nullifiers })))
}
/// Names owned by one owner key, including registrations still in the mempool.
async fn chain_names_by_owner(
    State(node): State<Node>,
    Query(query): Query<OwnerQuery>,
) -> ApiResult {
    let owner: [u8; 32] = hex::decode(&query.owner)
        .map_err(fail)?
        .try_into()
        .map_err(|_| fail(anyhow::anyhow!("owner must be 32 bytes of hex")))?;
    let d = node.data.lock().map_err(fail)?;
    let owned: Vec<Value> = d
        .chain
        .state
        .names
        .values()
        .filter(|entry| entry.owner == owner)
        .map(|entry| name_json(node.network, entry))
        .collect();
    let pending: Vec<String> = d
        .chain
        .mempool
        .values()
        .flat_map(|tx| names::operations(node.network, tx))
        .filter(|operation| operation.record.owner == owner)
        .map(|operation| operation.record.name)
        .collect();
    Ok(Json(json!({"names": owned, "pending": pending})))
}
async fn register(
    State(node): State<Node>,
    headers: HeaderMap,
    Json(bundle): Json<ReceiverBundle>,
) -> ApiResult {
    authorize(&node, &headers)?;
    let name = bundle.username.clone();
    let mut d = node.data.lock().map_err(fail)?;
    d.registry
        .register(bundle, node.network, unix_time())
        .map_err(fail)?;
    Ok(Json(json!({"username":name,"registered":true})))
}
fn rpc_limit_layer(gate: Arc<Semaphore>) -> tower::limit::GlobalConcurrencyLimitLayer {
    tower::limit::GlobalConcurrencyLimitLayer::with_semaphore(gate)
}
/// Requests per second the public RPC accepts in total, and the burst above it.
/// Onion clients have no address to limit individually, so the budget is global;
/// a light wallet syncing over Tor stays far below it.
const PUBLIC_RPC_RATE: f64 = 200.0;
const PUBLIC_RPC_BURST: f64 = 400.0;
/// A token bucket shared by every public RPC request.
#[derive(Clone)]
struct RateLimit(Arc<std::sync::Mutex<(f64, std::time::Instant)>>);
impl RateLimit {
    fn new() -> Self {
        Self(Arc::new(std::sync::Mutex::new((
            PUBLIC_RPC_BURST,
            std::time::Instant::now(),
        ))))
    }
    fn allow(&self) -> bool {
        let mut bucket = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = std::time::Instant::now();
        let refill = now.duration_since(bucket.1).as_secs_f64() * PUBLIC_RPC_RATE;
        bucket.0 = (bucket.0 + refill).min(PUBLIC_RPC_BURST);
        bucket.1 = now;
        if bucket.0 >= 1.0 {
            bucket.0 -= 1.0;
            true
        } else {
            false
        }
    }
}
async fn rate_limited(
    axum::extract::State(limit): axum::extract::State<RateLimit>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if limit.allow() {
        next.run(request).await
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "public RPC rate limit; retry shortly"})),
        )
            .into_response()
    }
}
pub async fn serve(node: Node, listen: SocketAddr, admin: bool) -> Result<()> {
    if admin {
        ensure!(listen.ip().is_loopback(), "admin RPC must bind to loopback");
    } else {
        ensure!(node.public_rpc, "public RPC requires explicit opt-in");
    }
    let mut router = Router::new()
        .route("/v1/info", get(info))
        .route("/v1/genesis", get(genesis))
        .route("/v1/supply", get(supply))
        .route("/v1/blocks", get(blocks))
        .route("/v1/usernames", get(resolve))
        .route("/v1/names", get(chain_name))
        .route("/v1/names/owner", get(chain_names_by_owner))
        .route("/v1/mempool/nullifiers", get(mempool_nullifiers))
        .route("/v1/relay", post(relay_tx));
    if admin {
        router = router
            .route("/v1/peers", get(peers))
            .route("/v1/network", get(network_status))
            .route("/v1/network/proxy", get(network_proxy))
            .route("/v1/network/reconnect", post(network_reconnect))
            .route("/v1/usernames/record", get(renewal_record))
            .route("/v1/mine/template", post(template))
            .route("/v1/mine/submit", post(submit_block))
            .route("/v1/transactions", post(submit_tx))
            .route("/v1/usernames", post(register));
    }
    let limits = tower::ServiceBuilder::new()
        .layer(axum::error_handling::HandleErrorLayer::new(
            |_: axum::BoxError| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "node busy or request timed out",
                )
            },
        ))
        .load_shed()
        .layer(rpc_limit_layer(if admin {
            node.rpc_gate.clone()
        } else {
            node.public_rpc_gate.clone()
        }))
        .timeout(std::time::Duration::from_secs(600));
    let mut router = router
        .layer(limits)
        .layer(DefaultBodyLimit::max(node.limits.rpc_request_bytes));
    if !admin {
        router = router.layer(axum::middleware::from_fn_with_state(
            RateLimit::new(),
            rate_limited,
        ));
    }
    let router = router.with_state(node);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen,admin,"RPC listening");
    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod diagnostics_tests {
    use super::*;

    #[test]
    fn database_measurement_excludes_wallets_and_unrelated_files() {
        let directory =
            std::env::temp_dir().join(format!("genoryn-size-{}", rand08::random::<u64>()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("chain.sqlite"), [0; 23]).unwrap();
        std::fs::write(directory.join("chain.sqlite-wal"), [0; 17]).unwrap();
        std::fs::write(directory.join("usernames.sqlite"), [0; 11]).unwrap();
        std::fs::write(directory.join("wallet.gnr"), [0; 100]).unwrap();
        assert_eq!(database_size(&directory).unwrap(), 51);
        std::fs::create_dir(directory.join("usernames.sqlite-shm")).unwrap();
        assert!(database_size(&directory).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn public_rpc_rate_limit_allows_a_burst_then_refills() {
        let limit = RateLimit::new();
        let allowed = (0..1000).filter(|_| limit.allow()).count();
        assert!(
            (PUBLIC_RPC_BURST as usize..PUBLIC_RPC_BURST as usize + 20).contains(&allowed),
            "{allowed}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(limit.allow(), "tokens refill over time");
    }

    #[test]
    fn rpc_budget_is_shared_across_routes_and_listeners() {
        use std::{
            future::{Pending, pending},
            task::{Context, Poll, Waker},
        };
        use tower::{Layer, Service};
        #[derive(Clone)]
        struct Handler;
        impl Service<()> for Handler {
            type Response = ();
            type Error = std::convert::Infallible;
            type Future = Pending<Result<(), Self::Error>>;
            fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }
            fn call(&mut self, _: ()) -> Self::Future {
                pending()
            }
        }
        let gate = Arc::new(Semaphore::new(1));
        let mut a = rpc_limit_layer(gate.clone()).layer(Handler);
        let mut b = rpc_limit_layer(gate).layer(Handler);
        let mut context = Context::from_waker(Waker::noop());
        assert!(a.poll_ready(&mut context).is_ready());
        let active = a.call(());
        assert!(b.poll_ready(&mut context).is_pending());
        drop(active);
        assert!(b.poll_ready(&mut context).is_ready());
    }
}
