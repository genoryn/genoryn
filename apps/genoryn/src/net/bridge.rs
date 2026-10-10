//! Loopback SOCKS5 front end to the built-in Tor client, so auxiliary HTTPS (the
//! published seed list, the wallet's update check) uses the same Tor client as
//! peer connections. It listens only on 127.0.0.1, requires username/password
//! authentication with random per-run credentials (returned only by the
//! token-protected admin RPC) and supports only CONNECT.
use super::{PeerStream, policy::TorTarget};
use anyhow::Result;
use futures::future::BoxFuture;
use rand08::RngCore;
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};
use zeroize::Zeroizing;

const MAX_CLIENTS: usize = 8;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub type Upstream =
    Arc<dyn Fn(TorTarget) -> BoxFuture<'static, Result<PeerStream, String>> + Send + Sync>;

pub struct AuxProxy {
    pub address: SocketAddr,
    pub username: String,
    pub password: Zeroizing<String>,
}
impl AuxProxy {
    /// `socks5h://user:pass@127.0.0.1:port` for HTTP clients; names stay remote.
    pub fn url(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "socks5h://{}:{}@{}",
            self.username,
            self.password.as_str(),
            self.address
        ))
    }
}

fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub async fn start(upstream: Upstream) -> Result<AuxProxy> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let mut secret = [0u8; 24];
    rand08::rngs::OsRng.fill_bytes(&mut secret);
    let proxy = AuxProxy {
        address,
        username: "genoryn".into(),
        password: Zeroizing::new(hex::encode(secret)),
    };
    let expected = Arc::new((proxy.username.clone(), proxy.password.clone()));
    let permits = Arc::new(Semaphore::new(MAX_CLIENTS));
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                continue;
            };
            let (expected, upstream) = (expected.clone(), upstream.clone());
            tokio::spawn(async move {
                let _permit = permit;
                let _ = serve(stream, &expected, upstream).await;
            });
        }
    });
    Ok(proxy)
}

async fn serve(
    mut client: TcpStream,
    expected: &(String, Zeroizing<String>),
    upstream: Upstream,
) -> std::io::Result<()> {
    let target =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut client, expected)).await {
            Ok(Ok(Some(target))) => target,
            _ => return Ok(()),
        };
    let reply = |code: u8| [0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    match upstream(target).await {
        Ok(mut remote) => {
            client.write_all(&reply(0x00)).await?;
            let _ = tokio::io::copy_bidirectional(&mut client, &mut remote).await;
        }
        Err(_) => client.write_all(&reply(0x04)).await?,
    }
    Ok(())
}

/// Returns the requested destination once the client has authenticated.
async fn handshake(
    client: &mut TcpStream,
    expected: &(String, Zeroizing<String>),
) -> std::io::Result<Option<TorTarget>> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    if head[0] != 0x05 || !methods.contains(&0x02) {
        client.write_all(&[0x05, 0xff]).await?;
        return Ok(None);
    }
    client.write_all(&[0x05, 0x02]).await?;
    let mut version = [0u8; 2];
    client.read_exact(&mut version).await?;
    let mut username = vec![0u8; version[1] as usize];
    client.read_exact(&mut username).await?;
    let mut length = [0u8; 1];
    client.read_exact(&mut length).await?;
    let mut password = Zeroizing::new(vec![0u8; length[0] as usize]);
    client.read_exact(&mut password).await?;
    let authenticated = (version[0] == 0x01)
        & equal(&username, expected.0.as_bytes())
        & equal(&password, expected.1.as_bytes());
    client
        .write_all(&[0x01, if authenticated { 0x00 } else { 0x01 }])
        .await?;
    if !authenticated {
        return Ok(None);
    }
    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    if request[0] != 0x05 || request[1] != 0x01 {
        client
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await?;
        return Ok(None);
    }
    let target = match request[3] {
        0x01 => {
            let mut bytes = [0u8; 6];
            client.read_exact(&mut bytes).await?;
            let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
            TorTarget::Ip(SocketAddr::new(
                ip.into(),
                u16::from_be_bytes([bytes[4], bytes[5]]),
            ))
        }
        0x04 => {
            let mut bytes = [0u8; 18];
            client.read_exact(&mut bytes).await?;
            let ip: [u8; 16] = bytes[..16].try_into().expect("fixed slice");
            TorTarget::Ip(SocketAddr::new(
                Ipv6Addr::from(ip).into(),
                u16::from_be_bytes([bytes[16], bytes[17]]),
            ))
        }
        0x03 => {
            let mut length = [0u8; 1];
            client.read_exact(&mut length).await?;
            let mut name = vec![0u8; length[0] as usize];
            client.read_exact(&mut name).await?;
            let mut port = [0u8; 2];
            client.read_exact(&mut port).await?;
            let Ok(name) = String::from_utf8(name) else {
                return Ok(None);
            };
            TorTarget::Domain(name, u16::from_be_bytes(port))
        }
        _ => {
            client
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(None);
        }
    };
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    async fn echo_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 64];
                    while let Ok(n) = stream.read(&mut buffer).await {
                        if n == 0 || stream.write_all(&buffer[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        address
    }

    #[tokio::test]
    async fn bridge_requires_credentials_and_forwards_names_unresolved() {
        let echo = echo_server().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let upstream: Upstream = Arc::new(move |target| {
            record.lock().unwrap().push(target);
            Box::pin(async move {
                TcpStream::connect(echo)
                    .await
                    .map(|stream| Box::pin(stream) as PeerStream)
                    .map_err(|error| error.to_string())
            })
        });
        let proxy = start(upstream).await.unwrap();
        assert!(proxy.address.ip().is_loopback());
        let url = proxy.url();
        assert!(url.starts_with("socks5h://genoryn:") && url.ends_with(&proxy.address.to_string()));

        // No credentials: refused before any destination is requested.
        let mut anonymous = TcpStream::connect(proxy.address).await.unwrap();
        anonymous.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut reply = [0u8; 2];
        anonymous.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0x05, 0xff]);

        // Wrong password: refused.
        let wrong = tokio_socks::tcp::Socks5Stream::connect_with_password(
            proxy.address,
            ("example.onion", 1),
            "genoryn",
            "wrong",
        )
        .await;
        assert!(wrong.is_err());
        assert!(
            seen.lock().unwrap().is_empty(),
            "nothing forwarded without auth"
        );

        // Correct credentials: the name reaches the upstream verbatim and data flows.
        let stream = tokio_socks::tcp::Socks5Stream::connect_with_password(
            proxy.address,
            ("seed.example.org", 443),
            &proxy.username,
            &proxy.password,
        )
        .await
        .unwrap();
        let mut stream = stream.into_inner();
        stream.write_all(b"ping").await.unwrap();
        let mut answer = [0u8; 4];
        stream.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"ping");
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [TorTarget::Domain("seed.example.org".into(), 443)]
        );
    }
}
