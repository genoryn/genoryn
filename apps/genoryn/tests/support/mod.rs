//! Controlled local stand-ins for a Tor daemon's SOCKS and control ports, plus
//! helpers for real loopback nodes. Nothing here contacts an external network.
#![allow(dead_code)]
use genoryn::{
    config::ResourceLimits,
    net::{NetworkMode, NetworkOptions, OnionV3, tor::TorConfig},
    node::Node,
};
use genoryn_core::params::Network;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocksTarget {
    Ip(SocketAddr),
    Domain(String, u16),
}
#[derive(Clone, Debug)]
pub struct SocksRequest {
    pub target: SocksTarget,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// A SOCKS5 server that records every request. Targets listed in `routes` are
/// forwarded to a local address; every other request gets `refusal`.
pub struct MockSocks {
    pub address: SocketAddr,
    pub requests: Arc<Mutex<Vec<SocksRequest>>>,
    pub routes: Arc<Mutex<BTreeMap<String, SocketAddr>>>,
    pub refusal: Arc<AtomicU8>,
    tasks: JoinSet<()>,
}
pub fn route_key(target: &SocksTarget) -> String {
    match target {
        SocksTarget::Ip(address) => address.to_string(),
        SocksTarget::Domain(host, port) => format!("{host}:{port}"),
    }
}
impl MockSocks {
    pub async fn start() -> Self {
        Self::start_at("127.0.0.1:0".parse().unwrap()).await
    }
    pub async fn start_at(address: SocketAddr) -> Self {
        let listener = bind_retrying(address).await;
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let routes = Arc::new(Mutex::new(BTreeMap::new()));
        let refusal = Arc::new(AtomicU8::new(0x04));
        let mut tasks = JoinSet::new();
        let (r, ro, re) = (requests.clone(), routes.clone(), refusal.clone());
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                connections.spawn(serve_socks(stream, r.clone(), ro.clone(), re.clone()));
            }
        });
        Self {
            address,
            requests,
            routes,
            refusal,
            tasks,
        }
    }
    pub fn route(&self, target: &str, to: SocketAddr) {
        self.routes.lock().unwrap().insert(target.to_owned(), to);
    }
    pub fn requests(&self) -> Vec<SocksRequest> {
        self.requests.lock().unwrap().clone()
    }
    /// Stops accepting and tears down every forwarded tunnel, like a Tor exit.
    pub fn shutdown(mut self) -> SocketAddr {
        self.tasks.abort_all();
        self.address
    }
}
impl Drop for MockSocks {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}
async fn bind_retrying(address: SocketAddr) -> TcpListener {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match TcpListener::bind(address).await {
            Ok(listener) => return listener,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("cannot bind {address}: {error}"),
        }
    }
}
async fn serve_socks(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<SocksRequest>>>,
    routes: Arc<Mutex<BTreeMap<String, SocketAddr>>>,
    refusal: Arc<AtomicU8>,
) {
    let result: std::io::Result<()> = async {
        let mut head = [0u8; 2];
        stream.read_exact(&mut head).await?;
        let mut methods = vec![0u8; head[1] as usize];
        stream.read_exact(&mut methods).await?;
        let (mut username, mut password) = (None, None);
        if methods.contains(&0x02) {
            stream.write_all(&[0x05, 0x02]).await?;
            let mut version = [0u8; 2];
            stream.read_exact(&mut version).await?;
            let mut user = vec![0u8; version[1] as usize];
            stream.read_exact(&mut user).await?;
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).await?;
            let mut pass = vec![0u8; length[0] as usize];
            stream.read_exact(&mut pass).await?;
            username = Some(String::from_utf8_lossy(&user).into_owned());
            password = Some(String::from_utf8_lossy(&pass).into_owned());
            stream.write_all(&[0x01, 0x00]).await?;
        } else {
            stream.write_all(&[0x05, 0x00]).await?;
        }
        let mut request = [0u8; 4];
        if stream.read_exact(&mut request).await.is_err() {
            return Ok(()); // a readiness probe closes after negotiating
        }
        let target = match request[3] {
            0x01 => {
                let mut bytes = [0u8; 6];
                stream.read_exact(&mut bytes).await?;
                SocksTarget::Ip(SocketAddr::from((
                    [bytes[0], bytes[1], bytes[2], bytes[3]],
                    u16::from_be_bytes([bytes[4], bytes[5]]),
                )))
            }
            0x04 => {
                let mut bytes = [0u8; 18];
                stream.read_exact(&mut bytes).await?;
                let ip: [u8; 16] = bytes[..16].try_into().unwrap();
                SocksTarget::Ip(SocketAddr::from((
                    ip,
                    u16::from_be_bytes([bytes[16], bytes[17]]),
                )))
            }
            0x03 => {
                let mut length = [0u8; 1];
                stream.read_exact(&mut length).await?;
                let mut name = vec![0u8; length[0] as usize];
                stream.read_exact(&mut name).await?;
                let mut port = [0u8; 2];
                stream.read_exact(&mut port).await?;
                SocksTarget::Domain(
                    String::from_utf8_lossy(&name).into_owned(),
                    u16::from_be_bytes(port),
                )
            }
            _ => return Ok(()),
        };
        let key = route_key(&target);
        requests.lock().unwrap().push(SocksRequest {
            target,
            username,
            password,
        });
        let route = routes.lock().unwrap().get(&key).copied();
        let reply = |code: u8| [0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        match route {
            Some(to) => match TcpStream::connect(to).await {
                Ok(mut upstream) => {
                    stream.write_all(&reply(0x00)).await?;
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
                }
                Err(_) => stream.write_all(&reply(0x05)).await?,
            },
            None => {
                stream
                    .write_all(&reply(refusal.load(Ordering::Relaxed)))
                    .await?
            }
        }
        Ok(())
    }
    .await;
    let _ = result;
}

/// A local TCP listener that only counts connections: proof that nothing dialed it.
pub struct Canary {
    pub address: SocketAddr,
    pub accepted: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Canary {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
        Self {
            address,
            accepted,
            task,
        }
    }
    pub fn count(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}
impl Drop for Canary {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn test_onion(seed: u8) -> OnionV3 {
    let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]).verifying_key();
    OnionV3::from_public_key(key.to_bytes()).unwrap()
}
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
pub fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

pub fn tor_config(socks: SocketAddr) -> TorConfig {
    let mut tor = TorConfig::new(Some(socks), 29410);
    tor.onion_target = loopback(free_port());
    tor
}
pub fn open_node(
    dir: &Path,
    listen: Option<SocketAddr>,
    mode: NetworkMode,
    tor: Option<TorConfig>,
) -> Node {
    let p2p = listen.unwrap_or_else(|| loopback(free_port()));
    Node::open_with_options(
        Network::Devnet,
        dir.to_path_buf(),
        p2p,
        false,
        ResourceLimits::default(),
        NetworkOptions { mode, tor, listen },
    )
    .unwrap()
}
pub async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    condition()
}
pub fn peer_count(node: &Node) -> usize {
    node.peers.lock().unwrap().len()
}

/// Tor control-port stand-in with real SAFECOOKIE and password checks.
pub struct MockControl {
    pub address: SocketAddr,
    pub cookie_path: PathBuf,
    pub commands: Arc<Mutex<Vec<String>>>,
    pub bootstrap: Arc<AtomicU8>,
    /// Forged SERVERHASH, to test that the client refuses an impostor.
    pub forge_server_hash: Arc<std::sync::atomic::AtomicBool>,
    pub sessions: Arc<AtomicUsize>,
    tasks: JoinSet<()>,
}
pub struct ControlOptions {
    pub methods: &'static str,
    pub password: Option<&'static str>,
    pub service: OnionV3,
    pub generated_key: &'static str,
}
impl MockControl {
    pub async fn start(dir: &Path, options: ControlOptions) -> Self {
        Self::start_at(dir, loopback(0), options).await
    }
    pub async fn start_at(dir: &Path, address: SocketAddr, options: ControlOptions) -> Self {
        let listener = bind_retrying(address).await;
        let address = listener.local_addr().unwrap();
        let cookie_path = dir.join("control_auth_cookie");
        if !cookie_path.exists() {
            std::fs::write(&cookie_path, [0x5au8; 32]).unwrap();
        }
        let commands = Arc::new(Mutex::new(Vec::new()));
        let bootstrap = Arc::new(AtomicU8::new(100));
        let forge = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();
        let state = ControlState {
            cookie: std::fs::read(&cookie_path).unwrap(),
            cookie_path: cookie_path.clone(),
            commands: commands.clone(),
            bootstrap: bootstrap.clone(),
            forge: forge.clone(),
            options: Arc::new(options),
        };
        let counter = sessions.clone();
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                connections.spawn(serve_control(stream, state.clone()));
            }
        });
        Self {
            address,
            cookie_path,
            commands,
            bootstrap,
            forge_server_hash: forge,
            sessions,
            tasks,
        }
    }
    pub fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
    pub fn shutdown(mut self) -> SocketAddr {
        self.tasks.abort_all();
        self.address
    }
}
impl Drop for MockControl {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}
#[derive(Clone)]
struct ControlState {
    cookie: Vec<u8>,
    cookie_path: PathBuf,
    commands: Arc<Mutex<Vec<String>>>,
    bootstrap: Arc<AtomicU8>,
    forge: Arc<std::sync::atomic::AtomicBool>,
    options: Arc<ControlOptions>,
}
fn hmac(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().to_vec()
}
async fn serve_control(stream: TcpStream, state: ControlState) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let mut authenticated = false;
    let mut challenge: Option<(Vec<u8>, Vec<u8>)> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        state.commands.lock().unwrap().push(line.clone());
        let reply = if line == "PROTOCOLINFO 1" {
            format!(
                "250-PROTOCOLINFO 1\r\n250-AUTH METHODS={} COOKIEFILE=\"{}\"\r\n250-VERSION Tor=\"0.4.8.99-mock\"\r\n250 OK\r\n",
                state.options.methods,
                state
                    .cookie_path
                    .display()
                    .to_string()
                    .replace('\\', "\\\\")
            )
        } else if let Some(nonce) = line.strip_prefix("AUTHCHALLENGE SAFECOOKIE ") {
            let client = hex::decode(nonce).unwrap_or_default();
            let server = vec![0x33u8; 32];
            let mut server_hash = hmac(
                b"Tor safe cookie authentication server-to-controller hash",
                &[&state.cookie, &client, &server],
            );
            if state.forge.load(Ordering::Relaxed) {
                server_hash[0] ^= 1;
            }
            challenge = Some((client, server.clone()));
            format!(
                "250 AUTHCHALLENGE SERVERHASH={} SERVERNONCE={}\r\n",
                hex::encode(server_hash),
                hex::encode(server)
            )
        } else if let Some(argument) = line.strip_prefix("AUTHENTICATE") {
            let argument = argument.trim();
            let accepted = if let Some(quoted) = argument.strip_prefix('"') {
                state
                    .options
                    .password
                    .is_some_and(|password| quoted.trim_end_matches('"') == password)
            } else if let Some((client, server)) = &challenge {
                hex::decode(argument).ok()
                    == Some(hmac(
                        b"Tor safe cookie authentication controller-to-server hash",
                        &[&state.cookie, client, server],
                    ))
            } else {
                false
            };
            authenticated = accepted;
            if accepted {
                "250 OK\r\n".into()
            } else {
                "515 Authentication failed\r\n".into()
            }
        } else if !authenticated {
            "514 Authentication required\r\n".into()
        } else if line == "GETINFO status/bootstrap-phase" {
            let progress = state.bootstrap.load(Ordering::Relaxed);
            format!(
                "250-status/bootstrap-phase=NOTICE BOOTSTRAP PROGRESS={progress} TAG=mock SUMMARY=\"Mock bootstrap\"\r\n250 OK\r\n"
            )
        } else if let Some(arguments) = line.strip_prefix("ADD_ONION ") {
            let service = state
                .options
                .service
                .hostname()
                .trim_end_matches(".onion")
                .to_owned();
            if arguments.starts_with("NEW:ED25519-V3") {
                format!(
                    "250-ServiceID={service}\r\n250-PrivateKey={}\r\n250 OK\r\n",
                    state.options.generated_key
                )
            } else {
                format!("250-ServiceID={service}\r\n250 OK\r\n")
            }
        } else {
            "510 Unrecognized command\r\n".into()
        };
        if writer.write_all(reply.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// Captures every tracing event emitted on this thread while the guard lives.
#[derive(Clone, Default)]
pub struct LogCapture(Arc<Mutex<Vec<u8>>>);
pub struct CaptureGuard {
    _scoped: tracing::subscriber::DefaultGuard,
    _sentinel: tracing::Dispatch,
}
impl LogCapture {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
    /// With a single registered dispatcher, tracing-core computes a new
    /// callsite's interest from whichever thread hits it first, so a callsite
    /// first reached on another thread would be disabled here too. A second
    /// registered dispatcher makes interest consider every dispatcher.
    pub fn install(&self) -> CaptureGuard {
        let sentinel = tracing::Dispatch::new(tracing_subscriber::registry());
        let capture = self.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || capture.clone())
            .finish();
        CaptureGuard {
            _scoped: tracing::subscriber::set_default(subscriber),
            _sentinel: sentinel,
        }
    }
}
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
