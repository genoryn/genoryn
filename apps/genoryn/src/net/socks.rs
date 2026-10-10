//! SOCKS5 client use through the maintained `tokio-socks` crate. Destination
//! names are always sent to the proxy (remote resolution); the proxy itself is
//! addressed only by IP literal, so no step here consults a DNS resolver.
use super::policy::TorTarget;
use rand08::RngCore;
use std::{fmt, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_socks::{TargetAddr, tcp::Socks5Stream};

#[derive(Debug)]
pub enum SocksError {
    /// Nothing accepted a connection at the proxy address.
    ProxyUnreachable,
    /// The proxy answered, but not as a usable SOCKS5 server.
    ProxyProtocol(String),
    /// The proxy (Tor) could not reach the destination.
    Refused(String),
    Timeout,
}
impl SocksError {
    /// The proxy itself is down or misbehaving, as opposed to one bad destination.
    pub fn proxy_down(&self) -> bool {
        matches!(self, Self::ProxyUnreachable | Self::ProxyProtocol(_))
    }
}
impl fmt::Display for SocksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProxyUnreachable => f.write_str("Tor SOCKS proxy unreachable"),
            Self::ProxyProtocol(reason) => write!(f, "Tor SOCKS proxy protocol error: {reason}"),
            Self::Refused(reason) => write!(f, "Tor could not reach the peer: {reason}"),
            Self::Timeout => f.write_str("Tor connection timed out"),
        }
    }
}
impl std::error::Error for SocksError {}

fn classify(error: tokio_socks::Error) -> SocksError {
    use tokio_socks::Error as E;
    match error {
        E::ProxyServerUnreachable => SocksError::ProxyUnreachable,
        E::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            SocksError::ProxyUnreachable
        }
        E::Io(error) => SocksError::ProxyProtocol(error.kind().to_string()),
        E::InvalidResponseVersion
        | E::NoAcceptableAuthMethods
        | E::InvalidReservedByte
        | E::UnknownAddressType
        | E::PasswordAuthFailure(_)
        | E::AuthorizationRequired => SocksError::ProxyProtocol(error.to_string()),
        // tokio-socks reports Tor's extended onion reply codes (0xF0-0xF7) this way.
        E::UnknownAuthMethod => SocksError::Refused("onion service unavailable or rejected".into()),
        other => SocksError::Refused(other.to_string()),
    }
}

/// Random credentials make Tor (IsolateSOCKSAuth, on by default) build a separate
/// circuit for each peer, so one exit or guard cannot trivially link them.
fn isolation_credentials() -> (String, String) {
    let mut bytes = [0u8; 16];
    rand08::rngs::OsRng.fill_bytes(&mut bytes);
    ("genoryn".into(), hex::encode(bytes))
}

pub async fn connect(
    proxy: SocketAddr,
    target: &TorTarget,
    isolate: bool,
    timeout: Duration,
) -> Result<TcpStream, SocksError> {
    let target = match target {
        TorTarget::Ip(address) => TargetAddr::Ip(*address),
        TorTarget::Domain(host, port) => TargetAddr::Domain(host.as_str().into(), *port),
    };
    let attempt = async {
        if isolate {
            let (username, password) = isolation_credentials();
            Socks5Stream::connect_with_password(proxy, target, &username, &password).await
        } else {
            Socks5Stream::connect(proxy, target).await
        }
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(Ok(stream)) => Ok(stream.into_inner()),
        Ok(Err(error)) => Err(classify(error)),
        Err(_) => Err(SocksError::Timeout),
    }
}

/// Readiness probe: a SOCKS5 method negotiation, then close. No destination is
/// requested, so the probe causes no Tor circuit or exit traffic.
pub async fn probe(proxy: SocketAddr, timeout: Duration) -> Result<(), SocksError> {
    let attempt = async {
        let mut stream = TcpStream::connect(proxy)
            .await
            .map_err(|_| SocksError::ProxyUnreachable)?;
        stream
            .write_all(&[0x05, 0x02, 0x00, 0x02])
            .await
            .map_err(|_| SocksError::ProxyUnreachable)?;
        let mut reply = [0u8; 2];
        stream
            .read_exact(&mut reply)
            .await
            .map_err(|_| SocksError::ProxyProtocol("no method selection reply".into()))?;
        if reply[0] != 0x05 || !matches!(reply[1], 0x00 | 0x02) {
            return Err(SocksError::ProxyProtocol(
                "not a SOCKS5 proxy accepting anonymous or isolated clients".into(),
            ));
        }
        Ok(())
    };
    // A proxy that cannot complete a two-byte negotiation in time is unusable.
    tokio::time::timeout(timeout, attempt)
        .await
        .unwrap_or(Err(SocksError::ProxyUnreachable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn probe_distinguishes_socks_from_other_services() {
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_address = closed.local_addr().unwrap();
        drop(closed);
        assert!(
            probe(closed_address, Duration::from_secs(2))
                .await
                .unwrap_err()
                .proxy_down()
        );

        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_address = http.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = http.accept().await.unwrap();
            let mut buffer = [0u8; 4];
            let _ = stream.read_exact(&mut buffer).await;
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        });
        assert!(matches!(
            probe(http_address, Duration::from_secs(2)).await,
            Err(SocksError::ProxyProtocol(_))
        ));
    }

    #[test]
    fn isolation_credentials_are_unique() {
        let (_, a) = isolation_credentials();
        let (_, b) = isolation_credentials();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
}
