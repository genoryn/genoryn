//! Shared mining: hashes the pool's jobs in this session's nonce range and
//! submits every hash that meets the share difficulty. The pool hashes each
//! share again before crediting it, and credits exactly its difficulty, so a
//! slow computer's work counts in proportion to what it really did.
use super::*;
use anyhow::ensure;
use genoryn_pool::protocol::{self, Job};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

struct Endpoint {
    tls: bool,
    host: String,
    port: u16,
}
fn endpoint(url: &str) -> Result<Endpoint> {
    let (tls, rest) = if let Some(rest) = url.strip_prefix("tls://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("tcp://") {
        (false, rest)
    } else {
        anyhow::bail!("a pool address starts with tcp:// or tls://");
    };
    let (host, port) = rest
        .rsplit_once(':')
        .context("a pool address needs host:port")?;
    let host = host.trim_matches(['[', ']']).to_owned();
    ensure!(!host.is_empty(), "a pool address needs a host");
    Ok(Endpoint {
        tls,
        host,
        port: port.parse().context("pool port")?,
    })
}
/// Plain connections are fine where nobody can rewrite them on the way:
/// this computer, a private network, or an onion service (Tor encrypts it).
fn plaintext_is_private(host: &str) -> bool {
    if genoryn::net::addr::is_onion_name(host) || host == "localhost" {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// The pool's proxy: `--pool-proxy`, else `GENORYN_POOL_PROXY` (how the wallet
/// passes its node's built-in Tor bridge, whose credentials stay out of argv).
fn proxy(args: &Args) -> Option<String> {
    args.pool_proxy
        .clone()
        .or_else(|| std::env::var("GENORYN_POOL_PROXY").ok())
        .filter(|proxy| !proxy.trim().is_empty())
}

async fn connect(args: &Args, target: &Endpoint) -> Result<Box<dyn Stream>> {
    let tcp: Box<dyn Stream> = match proxy(args) {
        Some(proxy) => {
            let rest = proxy.strip_prefix("socks5h://").context(
                "the pool proxy must be socks5h://host:port, so the proxy resolves names",
            )?;
            let destination = (target.host.as_str(), target.port);
            let stream = match rest.rsplit_once('@') {
                Some((credentials, address)) => {
                    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
                    tokio_socks::tcp::Socks5Stream::connect_with_password(
                        address,
                        destination,
                        user,
                        password,
                    )
                    .await
                }
                None => tokio_socks::tcp::Socks5Stream::connect(rest, destination).await,
            };
            Box::new(stream.context("the SOCKS proxy could not reach the pool")?)
        }
        None => {
            ensure!(
                !genoryn::net::addr::is_onion_name(&target.host),
                "an onion pool needs --pool-proxy socks5h://127.0.0.1:9050 (Tor); onion names are never resolved through system DNS"
            );
            let stream =
                tokio::net::TcpStream::connect((target.host.as_str(), target.port)).await?;
            stream.set_nodelay(true)?;
            Box::new(stream)
        }
    };
    if !target.tls {
        return Ok(tcp);
    }
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(target.host.clone())
        .context("the pool's TLS name")?;
    Ok(Box::new(
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .context("TLS to the pool failed")?,
    ))
}

/// The hashing threads working on one job.
struct Search {
    stop: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl Search {
    fn halt(self) {
        self.stop.store(true, Ordering::SeqCst);
        for worker in self.workers {
            let _ = worker.join();
        }
    }
}

fn start_search(
    job: &Job,
    threads: usize,
    dataset: Option<pow::FastDataset>,
    hashes: Arc<AtomicU64>,
    found: tokio::sync::mpsc::UnboundedSender<(String, u64)>,
) -> Result<Search> {
    let header = hex::decode(&job.header).context("job header")?;
    ensure!(
        header.len() >= protocol::NONCE_OFFSET + 8,
        "job header too short"
    );
    let seed: Hash = hex::decode(&job.seed)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .context("job seed")?;
    let start = protocol::parse_nonce(&job.nonce_start).context("job nonce range")?;
    let span = 1u64
        .checked_shl(u32::from(job.nonce_bits))
        .unwrap_or(u64::MAX);
    let difficulty = job.share_difficulty;
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for index in 0..threads {
        let (stop, hashes, found) = (stop.clone(), hashes.clone(), found.clone());
        let (mut header, dataset, job_id) = (header.clone(), dataset.clone(), job.job_id.clone());
        workers.push(std::thread::spawn(move || {
            let vm = dataset.as_ref().and_then(|d| d.vm().ok());
            let mut offset = index as u64;
            while !stop.load(Ordering::Relaxed) && offset < span {
                let nonce = start.wrapping_add(offset);
                header[protocol::NONCE_OFFSET..protocol::NONCE_OFFSET + 8]
                    .copy_from_slice(&nonce.to_le_bytes());
                let hash = match &vm {
                    Some(vm) => vm.hash(&header),
                    None => pow::randomx(seed, &header),
                };
                let Ok(hash) = hash else {
                    stop.store(true, Ordering::SeqCst);
                    return;
                };
                hashes.fetch_add(1, Ordering::Relaxed);
                if pow::meets_difficulty(hash, difficulty)
                    && found.send((job_id.clone(), nonce)).is_err()
                {
                    return;
                }
                offset += threads as u64;
            }
        }));
    }
    Ok(Search { stop, workers })
}

#[derive(Default)]
struct Counters {
    accepted: u64,
    rejected: u64,
    work: u64,
    blocks: u64,
    last_reject: Option<String>,
}

pub async fn run(args: &Args) -> Result<()> {
    let target = endpoint(args.pool.as_deref().unwrap_or_default()).context(Failure::Setup)?;
    if !target.tls
        && proxy(args).is_none()
        && !plaintext_is_private(&target.host)
        && !args.allow_plaintext
    {
        return Err(Failure::Setup.wrap(
            "an unencrypted pool connection over the public internet could be rewritten on the way; use tls://, an onion pool through Tor, or --allow-plaintext".into(),
        ));
    }
    let started = Instant::now();
    let hashes = Arc::new(AtomicU64::new(0));
    let fast = SharedFast::default();
    let mut retry = TEMPLATE_RETRY_START;
    let mut counters = Counters::default();
    loop {
        if args.max_seconds > 0 && started.elapsed().as_secs() >= args.max_seconds {
            break;
        }
        match session(args, &target, &fast, &hashes, &mut counters, started).await {
            Ok(()) => break,
            Err(error) => {
                eprintln!("pool connection lost, reconnecting in {retry:?}: {error:#}");
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(TEMPLATE_RETRY_MAX * 3);
            }
        }
    }
    report(args, &counters, &hashes, started, None, None);
    Ok(())
}

fn report(
    args: &Args,
    counters: &Counters,
    hashes: &AtomicU64,
    started: Instant,
    job: Option<&Job>,
    mode: Option<&str>,
) {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let total = hashes.load(Ordering::Relaxed);
    println!(
        "{}",
        json!({
            "event": "pool_mining",
            "pool": args.pool,
            "hashrate_hs": total as f64 / elapsed,
            "hashes": total,
            "accepted_shares": counters.accepted,
            "rejected_shares": counters.rejected,
            "accepted_work": counters.work,
            "block_candidates": counters.blocks,
            "last_reject": counters.last_reject,
            "share_difficulty": job.map(|j| j.share_difficulty),
            "network_difficulty": job.map(|j| j.network_difficulty),
            "height": job.map(|j| j.height),
            "threads": args.threads,
            "randomx_mode": mode,
            "elapsed_seconds": elapsed,
            "note": "Accepted shares are verified work recorded by the pool. They pay only when the pool finds a block, after 60 confirmations, by a pool payout.",
        })
    );
}

async fn session(
    args: &Args,
    target: &Endpoint,
    fast: &SharedFast,
    hashes: &Arc<AtomicU64>,
    counters: &mut Counters,
    started: Instant,
) -> Result<()> {
    let stream = connect(args, target).await?;
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    let login = json!({"id": 1, "method": "login", "params": {
        "address": args.wallet,
        "worker": args.worker.chars().take(64).collect::<String>(),
        "agent": concat!("genoryn-miner/", env!("CARGO_PKG_VERSION")),
    }});
    write.write_all(format!("{login}\n").as_bytes()).await?;
    let (found_tx, mut found_rx) = tokio::sync::mpsc::unbounded_channel::<(String, u64)>();
    let mut search: Option<Search> = None;
    let mut current: Option<Job> = None;
    let mut next_id = 2u64;
    let mut reported = Instant::now();
    let mut keepalive = tokio::time::interval(Duration::from_secs(60));
    let deadline = (args.max_seconds > 0)
        .then(|| tokio::time::Instant::from_std(started + Duration::from_secs(args.max_seconds)));
    let result: Result<()> = async {
        loop {
            let until_deadline = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                line = tokio::time::timeout(Duration::from_secs(300), lines.next_line()) => {
                    let line = line.context("the pool went silent")??.context("the pool closed the connection")?;
                    ensure!(line.len() <= protocol::MAX_LINE_BYTES * 4, "pool line too long");
                    let message: Value = serde_json::from_str(&line).context("pool sent malformed JSON")?;
                    if message["method"] == "job" {
                        let job: Job = serde_json::from_value(message["params"].clone())?;
                        ensure!(job.nonce_bits <= 63 && job.share_difficulty > 0, "pool sent an unusable job");
                        if let Some(old) = search.take() { old.halt(); }
                        let seed: Hash = hex::decode(&job.seed).ok().and_then(|b| b.try_into().ok()).context("job seed")?;
                        let dataset = match args.randomx_mode {
                            RandomXMode::Auto => fast_dataset(fast, seed),
                            RandomXMode::Light => None,
                        };
                        let mode = if dataset.is_some() { "fast" } else { "light" };
                        let threads = if dataset.is_some() { args.threads } else { args.threads.min(MAX_LIGHT_THREADS) };
                        search = Some(start_search(&job, threads, dataset, hashes.clone(), found_tx.clone())?);
                        current = Some(job);
                        report(args, counters, hashes, started, current.as_ref(), Some(mode));
                    } else if message["id"] == 1 {
                        if let Some(error) = message["error"].as_object() {
                            return Err(Failure::Setup.wrap(format!(
                                "the pool refused this miner: {}",
                                error.get("message").and_then(Value::as_str).unwrap_or("unknown reason")
                            )));
                        }
                        eprintln!("pool login accepted: {}", message["result"]);
                    } else if message["id"].as_u64().is_some_and(|id| id >= 2) {
                        if message["result"]["accepted"] == true {
                            counters.accepted += 1;
                            counters.work += message["result"]["receipt"]["work"].as_u64().unwrap_or(0);
                            if message["result"]["receipt"]["block_candidate"] == true {
                                counters.blocks += 1;
                            }
                        } else {
                            counters.rejected += 1;
                            counters.last_reject = message["error"]["message"].as_str().map(str::to_owned);
                        }
                    }
                }
                Some((job_id, nonce)) = found_rx.recv() => {
                    let submit = json!({"id": next_id, "method": "submit", "params": {"job_id": job_id, "nonce": protocol::format_nonce(nonce)}});
                    next_id += 1;
                    write.write_all(format!("{submit}\n").as_bytes()).await?;
                }
                _ = keepalive.tick() => {
                    write.write_all(b"{\"id\":0,\"method\":\"keepalive\"}\n").await?;
                }
                _ = until_deadline => return Ok(()),
            }
            if reported.elapsed() >= PROGRESS_EVERY {
                reported = Instant::now();
                report(args, counters, hashes, started, current.as_ref(), None);
            }
        }
    }
    .await;
    if let Some(search) = search.take() {
        search.halt();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_addresses_and_plaintext_policy() {
        let tls = endpoint("tls://pool.example.org:3333").unwrap();
        assert!(tls.tls && tls.host == "pool.example.org" && tls.port == 3333);
        let local = endpoint("tcp://127.0.0.1:29790").unwrap();
        assert!(!local.tls && plaintext_is_private(&local.host));
        assert!(endpoint("http://pool:1").is_err());
        assert!(endpoint("tcp://pool").is_err());
        assert!(endpoint("tcp://:1").is_err());
        assert!(plaintext_is_private("192.168.1.20"));
        assert!(plaintext_is_private("10.0.0.5"));
        assert!(!plaintext_is_private("203.0.113.7"));
        assert!(!plaintext_is_private("pool.example.org"));
        let v6 = endpoint("tcp://[::1]:29790").unwrap();
        assert!(plaintext_is_private(&v6.host));
    }
}
