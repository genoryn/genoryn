//! Tor for the node: by default the built-in client (`embedded`, Arti), or a Tor
//! daemon the operator runs, reached through its SOCKS and control ports.
//!
//! GENORYN never downloads, installs or launches a Tor program. With an external
//! daemon, control-port authentication uses SAFECOOKIE or a password read from a
//! file; the raw cookie is never sent (plain COOKIE authentication is refused).
//! Credentials and onion private keys are never logged or returned through RPC.
//! An external onion service is created without the `Detach` flag, so Tor removes
//! it as soon as this node's control connection ends.
use super::{
    PeerStream,
    addr::{OnionV3, PeerAddr},
    bridge::{self, AuxProxy},
    embedded::{EmbeddedTor, OnionHost},
    policy::TorTarget,
    socks,
    transport::ConnectError,
};
use anyhow::{Context, Result, bail, ensure};
use hmac::{Hmac, Mac};
use rand08::RngCore;
use serde::Serialize;
use sha2::Sha256;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Notify, mpsc},
};
use zeroize::Zeroizing;

pub const ONION_KEY_FILE: &str = "onion_v3_private_key";
/// Onion-service streams waiting for the P2P layer.
const ONION_QUEUE: usize = 16;
const MAX_REPLY_LINE: u64 = 8_192;
const MAX_REPLY_LINES: usize = 128;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const SERVER_TO_CONTROLLER: &[u8] = b"Tor safe cookie authentication server-to-controller hash";
const CONTROLLER_TO_SERVER: &[u8] = b"Tor safe cookie authentication controller-to-server hash";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorConfig {
    /// SOCKS port of an external Tor daemon (an IP literal, so reaching it never
    /// needs DNS). `None` selects the built-in Tor client.
    pub socks: Option<SocketAddr>,
    /// External daemon's control port, for bootstrap status and the onion service.
    pub control: Option<SocketAddr>,
    pub control_password_file: Option<PathBuf>,
    /// Overrides the cookie path Tor reports in PROTOCOLINFO.
    pub cookie_file: Option<PathBuf>,
    pub onion_service: bool,
    /// Loopback listener an external Tor forwards onion-service connections to.
    pub onion_target: SocketAddr,
    /// Port advertised with the onion address.
    pub onion_port: u16,
    /// Public RPC listener the built-in onion service also forwards to, on
    /// `onion_rpc_port`, so light wallets can reach this node over Tor.
    pub onion_rpc: Option<SocketAddr>,
    pub onion_rpc_port: u16,
    pub stream_isolation: bool,
}
impl TorConfig {
    pub fn new(socks: Option<SocketAddr>, base_port: u16) -> Self {
        Self {
            socks,
            control: None,
            control_password_file: None,
            cookie_file: None,
            onion_service: false,
            onion_target: SocketAddr::from(([127, 0, 0, 1], base_port.saturating_add(3))),
            onion_port: base_port,
            onion_rpc: None,
            onion_rpc_port: base_port.saturating_add(2),
            stream_isolation: true,
        }
    }
    pub fn built_in(&self) -> bool {
        self.socks.is_none()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.onion_port != 0, "onion-service port must be nonzero");
        let Some(socks) = self.socks else {
            ensure!(
                self.control.is_none()
                    && self.control_password_file.is_none()
                    && self.cookie_file.is_none(),
                "Tor control-port settings apply only to an external Tor (--tor-socks)"
            );
            return Ok(());
        };
        ensure!(socks.port() != 0, "Tor SOCKS port must be nonzero");
        if self.onion_service {
            ensure!(
                self.control.is_some(),
                "an external Tor's onion service needs its control port (--tor-control)"
            );
            ensure!(
                self.onion_target.ip().is_loopback() && self.onion_target.port() != 0,
                "the onion-service target must be a loopback address"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct OnionStatus {
    /// `disabled`, `starting`, `registered` or `failed`.
    pub state: String,
    /// Only exposed through the loopback administrative RPC.
    pub address: Option<String>,
    /// The public RPC through the same onion service, for light wallets.
    pub rpc_address: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TorStatus {
    /// `built-in` (Arti inside the node) or `external` (an operator-run daemon).
    pub backend: String,
    /// External: the SOCKS port answers. Built-in: the client is ready for traffic.
    pub socks_reachable: bool,
    /// `not configured`, `connecting`, `authenticated` or `failed`.
    pub control: String,
    pub control_error: Option<String>,
    pub auth_method: Option<String>,
    pub tor_version: Option<String>,
    pub bootstrap_progress: Option<u8>,
    pub bootstrap_summary: Option<String>,
    pub onion: OnionStatus,
    /// Outbound connections are attempted only while this is true.
    pub ready: bool,
    pub last_error: Option<String>,
    /// Control connections lost and re-established, e.g. after a Tor restart.
    pub reconnections: u32,
}

#[derive(Debug)]
struct Reply {
    code: u16,
    lines: Vec<String>,
}
impl Reply {
    fn ok(self, what: &str) -> Result<Self> {
        ensure!(
            self.code == 250,
            "Tor refused {what} (code {}): {}",
            self.code,
            sanitize(self.lines.last().map(String::as_str).unwrap_or(""))
        );
        Ok(self)
    }
    fn value(&self, key: &str) -> Option<&str> {
        self.lines
            .iter()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    }
}
/// Tor error text is shown to operators; keep it short and printable.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(160)
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProtocolInfo {
    pub methods: Vec<String>,
    pub cookie_file: Option<PathBuf>,
    pub version: Option<String>,
}

/// Parses a control-protocol QuotedString starting at `text`, returning the
/// unescaped value and the remaining input.
fn quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('"')?.char_indices();
    let mut value = String::new();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((value, &text[index + 2..])),
            '\\' => value.push(chars.next()?.1),
            c => value.push(c),
        }
    }
    None
}
fn parse_protocol_info(reply: &Reply) -> Result<ProtocolInfo> {
    let mut info = ProtocolInfo {
        methods: Vec::new(),
        cookie_file: None,
        version: None,
    };
    for line in &reply.lines {
        if let Some(auth) = line.strip_prefix("AUTH ") {
            let mut rest = auth.trim();
            while !rest.is_empty() {
                if let Some(methods) = rest.strip_prefix("METHODS=") {
                    let end = methods.find(' ').unwrap_or(methods.len());
                    info.methods = methods[..end].split(',').map(str::to_owned).collect();
                    rest = methods[end..].trim_start();
                } else if let Some(path) = rest.strip_prefix("COOKIEFILE=") {
                    let (value, remaining) = quoted(path).context("malformed COOKIEFILE")?;
                    info.cookie_file = Some(PathBuf::from(value));
                    rest = remaining.trim_start();
                } else {
                    let end = rest.find(' ').unwrap_or(rest.len());
                    rest = rest[end..].trim_start();
                }
            }
        } else if let Some(version) = line.strip_prefix("VERSION Tor=") {
            info.version = quoted(version).map(|(value, _)| sanitize(&value));
        }
    }
    ensure!(
        !info.methods.is_empty(),
        "Tor did not list authentication methods"
    );
    Ok(info)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bootstrap {
    pub progress: u8,
    pub tag: String,
    pub summary: String,
}
fn parse_bootstrap(value: &str) -> Result<Bootstrap> {
    let field = |key: &str| {
        value
            .split(' ')
            .find_map(|part| part.strip_prefix(key))
            .map(str::to_owned)
    };
    let progress = field("PROGRESS=")
        .and_then(|p| p.parse::<u8>().ok())
        .filter(|p| *p <= 100)
        .context("malformed bootstrap progress")?;
    let summary = value
        .find("SUMMARY=")
        .and_then(|index| quoted(&value[index + 8..]))
        .map(|(summary, _)| sanitize(&summary))
        .unwrap_or_default();
    Ok(Bootstrap {
        progress,
        tag: sanitize(&field("TAG=").unwrap_or_default()),
        summary,
    })
}

fn safecookie_hash(key: &[u8], cookie: &[u8], client: &[u8], server: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(cookie);
    mac.update(client);
    mac.update(server);
    mac
}
fn read_cookie(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let length = std::fs::metadata(path)
        .context("cannot read the Tor control cookie; check its permissions")?
        .len();
    ensure!(
        length == 32,
        "the Tor control cookie must be exactly 32 bytes"
    );
    let mut file = std::fs::File::open(path).context("cannot open the Tor control cookie")?;
    let mut cookie = Zeroizing::new(vec![0u8; 32]);
    std::io::Read::read_exact(&mut file, &mut cookie)?;
    Ok(cookie)
}
fn read_password(path: &Path) -> Result<Zeroizing<String>> {
    ensure!(
        std::fs::metadata(path)?.len() <= 1_024,
        "Tor control password file is too large"
    );
    let text = Zeroizing::new(std::fs::read_to_string(path)?);
    let password = Zeroizing::new(text.trim_end_matches(['\r', '\n']).to_owned());
    ensure!(
        !password.is_empty() && !password.contains(['\r', '\n', '\0']),
        "Tor control password must be a single nonempty line"
    );
    Ok(password)
}

/// A minimal, bounded client for the commands GENORYN needs.
pub struct ControlConnection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}
impl ControlConnection {
    pub async fn connect(address: SocketAddr) -> Result<Self> {
        let stream = tokio::time::timeout(CONTROL_TIMEOUT, TcpStream::connect(address))
            .await
            .context("Tor control port timed out")?
            .context("Tor control port unreachable")?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }
    async fn read_line(&mut self) -> Result<String> {
        let mut bytes = Vec::new();
        (&mut self.reader)
            .take(MAX_REPLY_LINE + 1)
            .read_until(b'\n', &mut bytes)
            .await?;
        ensure!(!bytes.is_empty(), "Tor closed the control connection");
        ensure!(
            bytes.len() as u64 <= MAX_REPLY_LINE && bytes.ends_with(b"\n"),
            "Tor control reply line too long"
        );
        let line = String::from_utf8(bytes).context("Tor control reply is not UTF-8")?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
    async fn read_reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        loop {
            ensure!(lines.len() < MAX_REPLY_LINES, "Tor control reply too long");
            let line = self.read_line().await?;
            ensure!(line.len() >= 4, "malformed Tor control reply");
            let code: u16 = line[..3].parse().context("malformed Tor reply code")?;
            let separator = line.as_bytes()[3];
            let text = line[4..].to_owned();
            match separator {
                b' ' => {
                    lines.push(text);
                    return Ok(Reply { code, lines });
                }
                b'-' => lines.push(text),
                b'+' => {
                    // Data reply: lines until a lone ".", joined to the key line.
                    let mut value = text;
                    for _ in 0..=MAX_REPLY_LINES {
                        let data = self.read_line().await?;
                        if data == "." {
                            lines.push(std::mem::take(&mut value));
                            break;
                        }
                        value.push('\n');
                        value.push_str(data.strip_prefix('.').unwrap_or(&data));
                    }
                    ensure!(value.is_empty(), "Tor control data reply too long");
                }
                _ => bail!("malformed Tor control reply"),
            }
        }
    }
    /// Sends one command line. Callers keep secrets in `Zeroizing` buffers.
    async fn command(&mut self, line: &str) -> Result<Reply> {
        ensure!(
            !line.contains(['\r', '\n']),
            "control command contains a line break"
        );
        tokio::time::timeout(CONTROL_TIMEOUT, async {
            let mut bytes = Zeroizing::new(Vec::with_capacity(line.len() + 2));
            bytes.extend_from_slice(line.as_bytes());
            bytes.extend_from_slice(b"\r\n");
            self.writer.write_all(&bytes).await?;
            self.read_reply().await
        })
        .await
        .context("Tor control command timed out")?
    }
    pub async fn protocol_info(&mut self) -> Result<ProtocolInfo> {
        let reply = self.command("PROTOCOLINFO 1").await?.ok("PROTOCOLINFO")?;
        parse_protocol_info(&reply)
    }
    /// Authenticates with a configured password, else SAFECOOKIE, else an
    /// unauthenticated (NULL) control port. Returns the method used.
    pub async fn authenticate(&mut self, config: &TorConfig) -> Result<(String, Option<String>)> {
        let info = self.protocol_info().await?;
        let offered = |method: &str| info.methods.iter().any(|m| m == method);
        if let Some(path) = &config.control_password_file {
            ensure!(
                offered("HASHEDPASSWORD"),
                "a Tor control password is configured but Tor does not accept passwords"
            );
            let password = read_password(path)?;
            let mut escaped = Zeroizing::new(String::with_capacity(password.len() + 16));
            escaped.push_str("AUTHENTICATE \"");
            for c in password.chars() {
                if matches!(c, '"' | '\\') {
                    escaped.push('\\');
                }
                escaped.push(c);
            }
            escaped.push('"');
            self.command(&escaped)
                .await?
                .ok("password authentication")
                .map_err(|_| anyhow::anyhow!("Tor rejected the control password"))?;
            return Ok(("password".into(), info.version));
        }
        if offered("SAFECOOKIE") {
            let path = config
                .cookie_file
                .clone()
                .or(info.cookie_file.clone())
                .context("Tor offered SAFECOOKIE without a cookie file")?;
            let mut client_nonce = [0u8; 32];
            rand08::rngs::OsRng.fill_bytes(&mut client_nonce);
            let reply = self
                .command(&format!(
                    "AUTHCHALLENGE SAFECOOKIE {}",
                    hex::encode(client_nonce)
                ))
                .await?
                .ok("AUTHCHALLENGE")?;
            let line = reply.lines.last().context("empty AUTHCHALLENGE reply")?;
            let field = |key: &str| {
                line.split(' ')
                    .find_map(|part| part.strip_prefix(key))
                    .and_then(|value| hex::decode(value).ok())
            };
            let server_hash = field("SERVERHASH=").context("missing SERVERHASH")?;
            let server_nonce = field("SERVERNONCE=").context("missing SERVERNONCE")?;
            ensure!(
                server_hash.len() == 32 && server_nonce.len() == 32,
                "malformed AUTHCHALLENGE reply"
            );
            let cookie = read_cookie(&path)?;
            // Tor must prove it knows the cookie before anything derived from it is sent.
            safecookie_hash(SERVER_TO_CONTROLLER, &cookie, &client_nonce, &server_nonce)
                .verify_slice(&server_hash)
                .map_err(|_| {
                    anyhow::anyhow!("Tor control port failed SAFECOOKIE server authentication")
                })?;
            let client_hash = Zeroizing::new(hex::encode(
                safecookie_hash(CONTROLLER_TO_SERVER, &cookie, &client_nonce, &server_nonce)
                    .finalize()
                    .into_bytes(),
            ));
            let command = Zeroizing::new(format!("AUTHENTICATE {}", client_hash.as_str()));
            self.command(&command)
                .await?
                .ok("SAFECOOKIE authentication")
                .map_err(|_| anyhow::anyhow!("Tor rejected SAFECOOKIE authentication"))?;
            return Ok(("safecookie".into(), info.version));
        }
        if offered("NULL") {
            self.command("AUTHENTICATE").await?.ok("authentication")?;
            return Ok(("none (unauthenticated control port)".into(), info.version));
        }
        bail!(
            "no supported Tor control authentication: configure a control password file or enable CookieAuthentication (SAFECOOKIE)"
        )
    }
    pub async fn bootstrap(&mut self) -> Result<Bootstrap> {
        let reply = self
            .command("GETINFO status/bootstrap-phase")
            .await?
            .ok("GETINFO")?;
        parse_bootstrap(
            reply
                .value("status/bootstrap-phase")
                .context("missing bootstrap phase")?,
        )
    }
    /// Registers the onion service. Returns its address and, for a new service,
    /// the private key Tor generated.
    pub async fn add_onion(
        &mut self,
        key: Option<&str>,
        virtual_port: u16,
        target: SocketAddr,
    ) -> Result<(OnionV3, Option<Zeroizing<String>>)> {
        let command = Zeroizing::new(format!(
            "ADD_ONION {} Port={virtual_port},{target}",
            key.unwrap_or("NEW:ED25519-V3")
        ));
        let reply = self.command(&command).await?.ok("ADD_ONION")?;
        let service = reply
            .value("ServiceID")
            .context("missing onion ServiceID")?;
        let onion = OnionV3::parse(&format!("{service}.onion"))?;
        let generated = reply
            .value("PrivateKey")
            .map(|value| Zeroizing::new(value.to_owned()));
        if let Some(generated) = &generated {
            ensure!(
                valid_onion_key(generated),
                "Tor returned a malformed onion key"
            );
        }
        Ok((onion, generated))
    }
}

fn valid_onion_key(text: &str) -> bool {
    text.strip_prefix("ED25519-V3:").is_some_and(|blob| {
        (1..=128).contains(&blob.len())
            && blob
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    })
}
/// The persistent onion identity. Readable only by its owner on Unix; on Windows
/// it inherits the user-profile ACL of the node data directory.
pub fn load_onion_key(data_dir: &Path) -> Result<Option<Zeroizing<String>>> {
    let path = data_dir.join(ONION_KEY_FILE);
    if !path.exists() {
        return Ok(None);
    }
    ensure!(
        std::fs::metadata(&path)?.len() <= 256,
        "onion key file is too large"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            std::fs::metadata(&path)?.permissions().mode() & 0o077 == 0,
            "onion key file permissions are too broad; chmod 600"
        );
    }
    let text = Zeroizing::new(std::fs::read_to_string(&path)?);
    let key = Zeroizing::new(text.trim().to_owned());
    ensure!(valid_onion_key(&key), "onion key file is malformed");
    Ok(Some(key))
}
pub fn store_onion_key(data_dir: &Path, key: &str) -> Result<()> {
    ensure!(
        valid_onion_key(key),
        "refusing to store a malformed onion key"
    );
    let path = data_dir.join(ONION_KEY_FILE);
    let temporary = data_dir.join(format!("{ONION_KEY_FILE}.tmp"));
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    std::io::Write::write_all(&mut file, key.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

/// Shared Tor readiness, updated by the monitor and read by the connector.
pub struct TorState {
    pub config: TorConfig,
    data_dir: PathBuf,
    status: RwLock<TorStatus>,
    wake: Notify,
    /// Signalled each time Tor becomes usable again, so peers are retried at
    /// once instead of after their backoff.
    pub became_ready: Notify,
    started: AtomicBool,
    engine: OnceLock<Arc<EmbeddedTor>>,
    aux: OnceLock<AuxProxy>,
    onion_sender: mpsc::Sender<PeerStream>,
    onion_receiver: Mutex<Option<mpsc::Receiver<PeerStream>>>,
}
impl TorState {
    pub fn new(config: TorConfig, data_dir: PathBuf) -> Arc<Self> {
        let status = TorStatus {
            backend: if config.built_in() {
                "built-in"
            } else {
                "external"
            }
            .into(),
            control: if config.control.is_some() {
                "connecting"
            } else {
                "not configured"
            }
            .into(),
            onion: OnionStatus {
                state: if config.onion_service {
                    "starting"
                } else {
                    "disabled"
                }
                .into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (onion_sender, onion_receiver) = mpsc::channel(ONION_QUEUE);
        Arc::new(Self {
            config,
            data_dir,
            status: RwLock::new(status),
            wake: Notify::new(),
            became_ready: Notify::new(),
            started: AtomicBool::new(false),
            engine: OnceLock::new(),
            aux: OnceLock::new(),
            onion_sender,
            onion_receiver: Mutex::new(Some(onion_receiver)),
        })
    }
    /// Starts the monitor once; later calls do nothing.
    pub fn start(self: &Arc<Self>) {
        if !self.started.swap(true, Ordering::AcqRel) {
            if self.config.built_in() {
                tokio::spawn(self.clone().run_built_in());
            } else {
                tokio::spawn(self.clone().run());
            }
        }
    }
    /// Inbound streams of the built-in onion service, taken once by the P2P layer.
    pub fn take_onion_inbound(&self) -> Option<mpsc::Receiver<PeerStream>> {
        self.onion_receiver.lock().ok()?.take()
    }
    /// SOCKS proxy URL for auxiliary HTTPS: the external daemon, or the built-in
    /// client's authenticated loopback bridge once it runs. Contains credentials.
    pub fn auxiliary_proxy(&self) -> Option<Zeroizing<String>> {
        match self.config.socks {
            Some(socks) => Some(Zeroizing::new(format!("socks5h://{socks}"))),
            None => self.aux.get().map(AuxProxy::url),
        }
    }
    /// One stream through Tor. Nothing is attempted while Tor is not ready, and a
    /// failure is reported, never retried over another transport.
    pub async fn connect(
        &self,
        target: &TorTarget,
        timeout: Duration,
    ) -> Result<PeerStream, ConnectError> {
        if !self.usable() {
            return Err(ConnectError::TorUnavailable);
        }
        match self.config.socks {
            Some(proxy) => {
                match socks::connect(proxy, target, self.config.stream_isolation, timeout).await {
                    Ok(stream) => Ok(super::tcp_stream(stream)),
                    Err(error) => {
                        if error.proxy_down() {
                            self.report_proxy_failure();
                        }
                        Err(ConnectError::Socks(error))
                    }
                }
            }
            None => {
                let engine = self.engine.get().ok_or(ConnectError::TorUnavailable)?;
                match tokio::time::timeout(
                    timeout,
                    engine.connect(target, self.config.stream_isolation),
                )
                .await
                {
                    Ok(Ok(stream)) => Ok(stream),
                    Ok(Err(error)) => Err(ConnectError::BuiltIn {
                        local: error.local,
                        message: error.message,
                    }),
                    Err(_) => Err(ConnectError::Timeout),
                }
            }
        }
    }
    /// Runs the built-in client for the life of the node: bootstrap (retried),
    /// readiness from its own status, the auxiliary bridge and the onion service.
    async fn run_built_in(self: Arc<Self>) {
        let engine = loop {
            match EmbeddedTor::create(&self.data_dir) {
                Ok(engine) => break engine,
                Err(error) => {
                    let text = sanitize(&format!("{error:#}"));
                    tracing::warn!(error = %text, "built-in Tor could not start; retrying");
                    self.update(|status| status.last_error = Some(text));
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                        _ = self.wake.notified() => {}
                    }
                }
            }
        };
        let _ = self.engine.set(engine.clone());
        let bridge_engine = engine.clone();
        let upstream: bridge::Upstream = Arc::new(move |target| {
            let engine = bridge_engine.clone();
            Box::pin(async move {
                engine
                    .connect(&target, true)
                    .await
                    .map_err(|error| error.message)
            })
        });
        match bridge::start(upstream).await {
            Ok(proxy) => {
                let _ = self.aux.set(proxy);
            }
            Err(error) => tracing::warn!(error = %error, "auxiliary Tor bridge unavailable"),
        }
        let booting = Arc::new(AtomicBool::new(false));
        let spawn_bootstrap = |state: Arc<Self>,
                               engine: Arc<EmbeddedTor>,
                               booting: Arc<AtomicBool>| {
            if booting.swap(true, Ordering::AcqRel) {
                return;
            }
            tokio::spawn(async move {
                loop {
                    match engine.bootstrap().await {
                        Ok(()) => break,
                        Err(error) => {
                            tracing::warn!(reason = %error.message, "built-in Tor bootstrap failed; retrying");
                            state.update(|status| {
                                status.last_error = Some(format!("bootstrap: {}", error.message))
                            });
                            tokio::time::sleep(Duration::from_secs(15)).await;
                        }
                    }
                }
                booting.store(false, Ordering::Release);
            });
        };
        spawn_bootstrap(self.clone(), engine.clone(), booting.clone());
        let mut onion: Option<OnionHost> = None;
        let mut events = engine.events();
        let mut was_ready = false;
        loop {
            let ready = engine.ready();
            let (progress, summary) = engine.progress();
            if self.config.onion_service && onion.is_none() && ready {
                let rpc = self
                    .config
                    .onion_rpc
                    .map(|target| (self.config.onion_rpc_port, target));
                match engine.launch_onion_service(
                    self.config.onion_port,
                    self.onion_sender.clone(),
                    rpc,
                ) {
                    Ok(host) => onion = Some(host),
                    Err(error) => {
                        let text = sanitize(&format!("{error:#}"));
                        tracing::warn!(error = %text, "onion service could not start");
                        self.update(|status| {
                            status.onion.state = "failed".into();
                            status.onion.error = Some(text);
                        });
                    }
                }
            }
            let onion_state = onion.as_ref().map(|host| (host.state(), host.hostname()));
            self.update(|status| {
                status.socks_reachable = ready;
                status.bootstrap_progress = Some(if ready { 100 } else { progress.min(99) });
                status.bootstrap_summary = Some(summary);
                if let Some(((state, error), hostname)) = onion_state {
                    status.onion.state = state.into();
                    status.onion.error = error;
                    let key = hostname.and_then(|name| OnionV3::parse(&name).ok());
                    status.onion.address =
                        key.map(|key| PeerAddr::onion(key, self.config.onion_port).to_string());
                    status.onion.rpc_address = key
                        .filter(|_| self.config.onion_rpc.is_some())
                        .map(|key| PeerAddr::onion(key, self.config.onion_rpc_port).to_string());
                }
            });
            if ready != was_ready {
                if ready {
                    tracing::info!("built-in Tor is ready; outbound peer connections enabled");
                } else {
                    tracing::warn!("built-in Tor is not ready; Tor-routed peer connections paused");
                }
                was_ready = ready;
            }
            tokio::select! {
                _ = futures::StreamExt::next(&mut events) => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = self.wake.notified() => {
                    spawn_bootstrap(self.clone(), engine.clone(), booting.clone());
                }
            }
        }
    }
    pub fn usable(&self) -> bool {
        self.status.read().is_ok_and(|status| status.ready)
    }
    pub fn snapshot(&self) -> TorStatus {
        self.status
            .read()
            .map(|status| status.clone())
            .unwrap_or_default()
    }
    pub fn onion_address(&self) -> Option<PeerAddr> {
        let status = self.status.read().ok()?;
        if status.onion.state != "registered" {
            return None;
        }
        status.onion.address.as_deref()?.parse().ok()
    }
    /// With a usable control port, Tor is ready only once it reports a complete
    /// bootstrap. Without one (not configured, or failed and reported as such),
    /// a SOCKS proxy that answers is the only available signal.
    fn update(&self, change: impl FnOnce(&mut TorStatus)) {
        if let Ok(mut status) = self.status.write() {
            let was_ready = status.ready;
            change(&mut status);
            status.ready = status.socks_reachable
                && match status.control.as_str() {
                    "not configured" | "failed" => true,
                    _ => status.bootstrap_progress == Some(100),
                };
            if status.ready && !was_ready {
                status.last_error = None;
                self.became_ready.notify_one();
            }
        }
    }
    /// Called when a connection through the proxy fails at the proxy itself:
    /// stop dialing at once and re-check sooner.
    pub fn report_proxy_failure(&self) {
        self.update(|status| {
            status.socks_reachable = false;
            status.last_error = Some("Tor SOCKS proxy stopped answering".into());
        });
        self.wake.notify_one();
    }
    pub fn request_reconnect(&self) {
        self.wake.notify_one();
    }
    async fn open_session(&self, address: SocketAddr) -> Result<ControlConnection> {
        let mut connection = ControlConnection::connect(address).await?;
        let (method, version) = connection.authenticate(&self.config).await?;
        tracing::info!(method = %method, "Tor control port authenticated");
        self.update(|status| {
            status.control = "authenticated".into();
            status.control_error = None;
            status.auth_method = Some(method);
            status.tor_version = version;
        });
        Ok(connection)
    }
    async fn publish_onion(&self, connection: &mut ControlConnection) -> Result<OnionV3> {
        let stored = load_onion_key(&self.data_dir)?;
        let (onion, generated) = connection
            .add_onion(
                stored.as_deref().map(String::as_str),
                self.config.onion_port,
                self.config.onion_target,
            )
            .await?;
        if let Some(generated) = generated {
            store_onion_key(&self.data_dir, &generated)?;
        }
        Ok(onion)
    }
    /// Monitors the SOCKS and control ports for the life of the node. Lost control
    /// connections (for example a Tor restart) are re-established and the onion
    /// service is re-registered with the same stored key.
    pub async fn run(self: Arc<Self>) {
        let mut session: Option<ControlConnection> = None;
        let mut onion_registered = false;
        let mut ever_connected = false;
        let mut was_ready = false;
        let Some(proxy) = self.config.socks else {
            return;
        };
        loop {
            let socks = socks::probe(proxy, Duration::from_secs(5)).await;
            self.update(|status| {
                status.socks_reachable = socks.is_ok();
                if let Err(error) = &socks {
                    status.last_error = Some(error.to_string());
                }
            });
            if let Some(address) = self.config.control {
                if session.is_none() {
                    match self.open_session(address).await {
                        Ok(connection) => {
                            if ever_connected {
                                self.update(|status| status.reconnections += 1);
                            }
                            ever_connected = true;
                            session = Some(connection);
                        }
                        Err(error) => {
                            let text = sanitize(&format!("{error:#}"));
                            tracing::warn!(error = %text, "Tor control port unavailable");
                            self.update(|status| {
                                status.control = "failed".into();
                                status.control_error = Some(text);
                                status.bootstrap_progress = None;
                                status.bootstrap_summary = None;
                            });
                        }
                    }
                }
                if let Some(connection) = session.as_mut() {
                    match connection.bootstrap().await {
                        Ok(bootstrap) => self.update(|status| {
                            status.bootstrap_progress = Some(bootstrap.progress);
                            status.bootstrap_summary = Some(bootstrap.summary);
                        }),
                        Err(error) => {
                            tracing::warn!("Tor control connection lost; reconnecting");
                            session = None;
                            onion_registered = false;
                            let text = sanitize(&format!("{error:#}"));
                            self.update(|status| {
                                status.control = "connecting".into();
                                status.control_error = Some(text);
                                status.bootstrap_progress = Some(0);
                                status.bootstrap_summary = None;
                                if self.config.onion_service {
                                    status.onion.state = "starting".into();
                                }
                            });
                        }
                    }
                }
                if self.config.onion_service
                    && !onion_registered
                    && let Some(connection) = session.as_mut()
                {
                    match self.publish_onion(connection).await {
                        Ok(onion) => {
                            onion_registered = true;
                            tracing::info!("onion service registered with Tor");
                            let address = PeerAddr::onion(onion, self.config.onion_port);
                            self.update(|status| {
                                status.onion = OnionStatus {
                                    state: "registered".into(),
                                    address: Some(address.to_string()),
                                    rpc_address: None,
                                    error: None,
                                };
                            });
                        }
                        Err(error) => {
                            let text = sanitize(&format!("{error:#}"));
                            tracing::warn!(error = %text, "onion service registration failed");
                            self.update(|status| {
                                status.onion.state = "failed".into();
                                status.onion.error = Some(text);
                            });
                        }
                    }
                }
            }
            let ready = self.usable();
            if ready != was_ready {
                if ready {
                    tracing::info!("Tor is ready; outbound peer connections enabled");
                } else {
                    tracing::warn!("Tor is not ready; Tor-routed peer connections paused");
                }
                was_ready = ready;
            }
            let interval = if ready {
                Duration::from_secs(15)
            } else {
                Duration::from_secs(2)
            };
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = self.wake.notified() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(lines: &[&str]) -> Reply {
        Reply {
            code: 250,
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        }
    }

    #[test]
    fn protocol_info_parses_methods_cookie_and_version() {
        let info = parse_protocol_info(&reply(&[
            "PROTOCOLINFO 1",
            r#"AUTH METHODS=COOKIE,SAFECOOKIE,HASHEDPASSWORD COOKIEFILE="C:\\Tor\\data\\control_auth_cookie""#,
            r#"VERSION Tor="0.4.8.12""#,
            "OK",
        ]))
        .unwrap();
        assert_eq!(info.methods, ["COOKIE", "SAFECOOKIE", "HASHEDPASSWORD"]);
        assert_eq!(
            info.cookie_file,
            Some(PathBuf::from(r"C:\Tor\data\control_auth_cookie"))
        );
        assert_eq!(info.version.as_deref(), Some("0.4.8.12"));
        assert!(parse_protocol_info(&reply(&["PROTOCOLINFO 1", "OK"])).is_err());
    }

    #[test]
    fn bootstrap_phase_is_parsed_and_bounded() {
        let parsed = parse_bootstrap(
            r#"NOTICE BOOTSTRAP PROGRESS=85 TAG=ap_conn_done SUMMARY="Connected to a relay to build circuits""#,
        )
        .unwrap();
        assert_eq!(parsed.progress, 85);
        assert_eq!(parsed.tag, "ap_conn_done");
        assert_eq!(parsed.summary, "Connected to a relay to build circuits");
        assert!(parse_bootstrap("NOTICE BOOTSTRAP PROGRESS=101 TAG=x").is_err());
        assert!(parse_bootstrap("NOTICE BOOTSTRAP TAG=x").is_err());
    }

    #[test]
    fn onion_keys_are_validated_and_stored_privately() {
        let directory = tempfile::tempdir().unwrap();
        assert!(load_onion_key(directory.path()).unwrap().is_none());
        let key = format!("ED25519-V3:{}", "A".repeat(86) + "==");
        store_onion_key(directory.path(), &key).unwrap();
        assert_eq!(
            load_onion_key(directory.path()).unwrap().unwrap().as_str(),
            key
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join(ONION_KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
        for bad in [
            "",
            "RSA1024:abc",
            "ED25519-V3:",
            "ED25519-V3:a b",
            "ED25519-V3:x\nPort=1",
        ] {
            assert!(store_onion_key(directory.path(), bad).is_err(), "{bad:?}");
        }
        std::fs::write(directory.path().join(ONION_KEY_FILE), "garbage").unwrap();
        assert!(load_onion_key(directory.path()).is_err());
    }

    fn external() -> Option<SocketAddr> {
        Some("127.0.0.1:9050".parse().unwrap())
    }

    #[test]
    fn configuration_requires_control_for_onion_service() {
        let mut config = TorConfig::new(external(), 29610);
        assert!(config.validate().is_ok());
        assert_eq!(config.onion_target, "127.0.0.1:29613".parse().unwrap());
        config.onion_service = true;
        assert!(config.validate().is_err());
        config.control = Some("127.0.0.1:9051".parse().unwrap());
        assert!(config.validate().is_ok());
        config.onion_target = "0.0.0.0:29613".parse().unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn built_in_tor_needs_no_ports_and_refuses_control_settings() {
        let mut config = TorConfig::new(None, 29610);
        assert!(config.built_in());
        config.onion_service = true;
        assert!(
            config.validate().is_ok(),
            "built-in onion services need no control port"
        );
        config.control = Some("127.0.0.1:9051".parse().unwrap());
        assert!(config.validate().is_err());
        config.control = None;
        config.control_password_file = Some("password".into());
        assert!(config.validate().is_err());
        let state = TorState::new(TorConfig::new(None, 29610), std::env::temp_dir());
        assert_eq!(state.snapshot().backend, "built-in");
        assert!(
            !state.usable(),
            "not ready before the built-in client bootstraps"
        );
        assert!(
            state.auxiliary_proxy().is_none(),
            "no bridge before the client runs"
        );
        let external_state = TorState::new(TorConfig::new(external(), 29610), std::env::temp_dir());
        assert_eq!(external_state.snapshot().backend, "external");
        assert_eq!(
            external_state.auxiliary_proxy().unwrap().as_str(),
            "socks5h://127.0.0.1:9050"
        );
    }

    #[tokio::test]
    async fn no_stream_is_attempted_before_tor_is_ready() {
        for socks in [None, external()] {
            let state = TorState::new(TorConfig::new(socks, 29610), std::env::temp_dir());
            let target = TorTarget::Domain("seed.example.org".into(), 29610);
            assert!(matches!(
                state.connect(&target, Duration::from_secs(1)).await,
                Err(ConnectError::TorUnavailable)
            ));
        }
    }

    #[test]
    fn readiness_requires_proxy_and_complete_bootstrap() {
        let socks_only = TorState::new(TorConfig::new(external(), 29610), std::env::temp_dir());
        assert!(!socks_only.usable());
        socks_only.update(|status| status.socks_reachable = true);
        assert!(
            socks_only.usable(),
            "without a control port SOCKS is the signal"
        );
        socks_only.report_proxy_failure();
        assert!(!socks_only.usable());

        let mut config = TorConfig::new(external(), 29610);
        config.control = Some("127.0.0.1:9051".parse().unwrap());
        let state = TorState::new(config, std::env::temp_dir());
        state.update(|status| status.socks_reachable = true);
        assert!(
            !state.usable(),
            "bootstrap unknown while the control port connects"
        );
        state.update(|status| {
            status.control = "authenticated".into();
            status.bootstrap_progress = Some(40);
        });
        assert!(!state.usable());
        state.update(|status| status.bootstrap_progress = Some(100));
        assert!(state.usable());
        state.update(|status| {
            status.control = "failed".into();
            status.bootstrap_progress = None;
        });
        assert!(
            state.usable(),
            "a failed control port degrades to SOCKS readiness"
        );
        state.report_proxy_failure();
        assert!(!state.usable());
    }
}
