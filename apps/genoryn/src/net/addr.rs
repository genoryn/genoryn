//! Peer addresses across clearnet and Tor. Parsing never performs DNS resolution:
//! `.onion` names become onion addresses or errors, never resolver queries.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha3::{Digest, Sha3_256};
use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    str::FromStr,
};

const ONION_LABEL_LEN: usize = 56;
const ONION_VERSION: u8 = 3;
const MAX_HOSTNAME_LEN: usize = 253;

/// A Tor v3 onion service identity: its ed25519 public key (rend-spec-v3 §6).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OnionV3([u8; 32]);

fn onion_checksum(key: &[u8; 32]) -> [u8; 2] {
    let mut digest = Sha3_256::new();
    digest.update(b".onion checksum");
    digest.update(key);
    digest.update([ONION_VERSION]);
    let hash = digest.finalize();
    [hash[0], hash[1]]
}

impl OnionV3 {
    /// Accepts only keys that decode to an ed25519 point of large order.
    pub fn from_public_key(key: [u8; 32]) -> Result<Self> {
        let point = ed25519_dalek::VerifyingKey::from_bytes(&key)
            .map_err(|_| anyhow::anyhow!("onion key is not an ed25519 point"))?;
        ensure!(!point.is_weak(), "onion key has small order");
        Ok(Self(key))
    }
    /// Parses `<56 base32 characters>.onion`. Subdomains, v2 names, wrong
    /// versions, bad checksums and invalid keys are rejected.
    pub fn parse(host: &str) -> Result<Self> {
        let host = host.to_ascii_lowercase();
        let label = host
            .strip_suffix(".onion")
            .context("onion address must end in .onion")?;
        ensure!(
            label.len() == ONION_LABEL_LEN
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)),
            "onion address must be a 56-character v3 name"
        );
        let decoded = data_encoding::BASE32_NOPAD
            .decode(label.to_ascii_uppercase().as_bytes())
            .map_err(|_| anyhow::anyhow!("onion address is not base32"))?;
        ensure!(decoded.len() == 35, "onion address has the wrong length");
        ensure!(
            decoded[34] == ONION_VERSION,
            "only v3 onion services are supported"
        );
        let key: [u8; 32] = decoded[..32].try_into().expect("fixed slice length");
        ensure!(
            decoded[32..34] == onion_checksum(&key),
            "onion address checksum mismatch"
        );
        Self::from_public_key(key)
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.0
    }
    pub fn hostname(&self) -> String {
        let mut bytes = Vec::with_capacity(35);
        bytes.extend_from_slice(&self.0);
        bytes.extend_from_slice(&onion_checksum(&self.0));
        bytes.push(ONION_VERSION);
        format!(
            "{}.onion",
            data_encoding::BASE32_NOPAD
                .encode(&bytes)
                .to_ascii_lowercase()
        )
    }
}
impl fmt::Debug for OnionV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OnionV3({})", self.hostname())
    }
}

/// Coarse network class, used for routing, diversity and status reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetClass {
    Ipv4,
    Ipv6,
    Onion,
    Name,
}
impl NetClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::Onion => "onion",
            Self::Name => "name",
        }
    }
}

/// Where a peer can be reached. `Name` exists only for operator-configured
/// bootstrap entries; it is never learned from, or advertised to, peers.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PeerAddr {
    Ip(SocketAddr),
    Onion { key: OnionV3, port: u16 },
    Name { host: String, port: u16 },
}

pub fn normalized_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        ip => ip,
    }
}

/// RFC 1123 host name: LDH labels of 1–63 bytes, no leading/trailing hyphen,
/// at most 253 bytes, and not an all-numeric dotted string (an IP typo).
pub fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOSTNAME_LEN
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && !host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

pub fn is_onion_name(host: &str) -> bool {
    host.to_ascii_lowercase()
        .trim_end_matches('.')
        .ends_with(".onion")
}

impl PeerAddr {
    pub fn onion(key: OnionV3, port: u16) -> Self {
        Self::Onion { key, port }
    }
    pub fn port(&self) -> u16 {
        match self {
            Self::Ip(address) => address.port(),
            Self::Onion { port, .. } | Self::Name { port, .. } => *port,
        }
    }
    pub fn class(&self) -> NetClass {
        match self {
            Self::Ip(address) => match normalized_ip(address.ip()) {
                IpAddr::V4(_) => NetClass::Ipv4,
                IpAddr::V6(_) => NetClass::Ipv6,
            },
            Self::Onion { .. } => NetClass::Onion,
            Self::Name { .. } => NetClass::Name,
        }
    }
    pub fn is_onion(&self) -> bool {
        matches!(self, Self::Onion { .. })
    }
    pub fn ip(&self) -> Option<SocketAddr> {
        match self {
            Self::Ip(address) => Some(*address),
            _ => None,
        }
    }
    /// Parses `host:port`; a bare host takes `default_port`. Never resolves DNS.
    pub fn parse_with_default_port(text: &str, default_port: u16) -> Result<Self> {
        let text = text.trim();
        ensure!(
            !text.is_empty() && text.len() <= 300,
            "invalid peer address"
        );
        if let Ok(address) = SocketAddr::from_str(text) {
            return Self::from_socket(address);
        }
        if let Ok(ip) = IpAddr::from_str(text) {
            return Self::from_socket(SocketAddr::new(ip, default_port));
        }
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (
                host,
                port.parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .context("invalid peer port")?,
            ),
            Some(_) => bail!("IPv6 peer addresses need brackets and a port"),
            None => (text, default_port),
        };
        ensure!(default_port != 0 || port != 0, "invalid peer port");
        if is_onion_name(host) {
            return Ok(Self::Onion {
                key: OnionV3::parse(host.trim_end_matches('.'))?,
                port,
            });
        }
        let host = host.to_ascii_lowercase();
        ensure!(valid_hostname(&host), "invalid peer host name");
        Ok(Self::Name { host, port })
    }
    fn from_socket(address: SocketAddr) -> Result<Self> {
        ensure!(address.port() != 0, "invalid peer port");
        Ok(Self::Ip(SocketAddr::new(
            normalized_ip(address.ip()),
            address.port(),
        )))
    }
}
impl FromStr for PeerAddr {
    type Err = anyhow::Error;
    fn from_str(text: &str) -> Result<Self> {
        Self::parse_with_default_port(text, 0)
    }
}
impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(address) => write!(f, "{address}"),
            Self::Onion { key, port } => write!(f, "{}:{port}", key.hostname()),
            Self::Name { host, port } => write!(f, "{host}:{port}"),
        }
    }
}
impl fmt::Debug for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerAddr({self})")
    }
}
impl From<SocketAddr> for PeerAddr {
    fn from(address: SocketAddr) -> Self {
        Self::Ip(SocketAddr::new(normalized_ip(address.ip()), address.port()))
    }
}
/// Serialized as text, so a version-1 `peers.json` of socket addresses still loads.
impl Serialize for PeerAddr {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for PeerAddr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Compact address-exchange encoding, sent only to peers that announced support.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireAddr {
    V4([u8; 4], u16),
    V6([u8; 16], u16),
    OnionV3([u8; 32], u16),
}
impl WireAddr {
    pub fn from_peer(address: &PeerAddr) -> Option<Self> {
        match address {
            PeerAddr::Ip(socket) => match normalized_ip(socket.ip()) {
                IpAddr::V4(ip) => Some(Self::V4(ip.octets(), socket.port())),
                IpAddr::V6(ip) => Some(Self::V6(ip.octets(), socket.port())),
            },
            PeerAddr::Onion { key, port } => Some(Self::OnionV3(key.public_key(), *port)),
            PeerAddr::Name { .. } => None,
        }
    }
    pub fn to_peer(&self) -> Result<PeerAddr> {
        let address = match self {
            Self::V4(ip, port) => {
                PeerAddr::from(SocketAddr::new(Ipv4Addr::from(*ip).into(), *port))
            }
            Self::V6(ip, port) => {
                PeerAddr::from(SocketAddr::new(Ipv6Addr::from(*ip).into(), *port))
            }
            Self::OnionV3(key, port) => PeerAddr::Onion {
                key: OnionV3::from_public_key(*key)?,
                port: *port,
            },
        };
        ensure!(address.port() != 0, "advertised address has port zero");
        Ok(address)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A valid v3 address derived from a fixed public test key.
    pub(crate) fn test_onion(seed: u8) -> OnionV3 {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]).verifying_key();
        OnionV3::from_public_key(key.to_bytes()).unwrap()
    }

    #[test]
    fn onion_round_trip_and_known_vector() {
        // Tor Project's published v3 service (duckduckgo), a public test vector.
        let known = "duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion";
        let parsed = OnionV3::parse(known).unwrap();
        assert_eq!(parsed.hostname(), known);
        assert_eq!(OnionV3::parse(&known.to_uppercase()).unwrap(), parsed);
        let generated = test_onion(7);
        assert_eq!(OnionV3::parse(&generated.hostname()).unwrap(), generated);
    }

    #[test]
    fn malformed_onions_are_rejected_without_becoming_names() {
        let good = test_onion(9).hostname();
        let mut checksum = good.clone().into_bytes();
        // Flip one character inside the checksum region (characters 51-54).
        checksum[52] = if checksum[52] == b'a' { b'b' } else { b'a' };
        let checksum = String::from_utf8(checksum).unwrap();
        for bad in [
            "expyuzz4wqqyqhjn.onion".to_owned(),
            checksum,
            format!("sub.{good}"),
            format!("{good}."),
            format!("{}.onion", "a".repeat(56)),
            format!("{}0.onion", &good[..55]),
        ] {
            assert!(OnionV3::parse(&bad).is_err(), "{bad}");
            let peer = PeerAddr::parse_with_default_port(&bad, 29610);
            assert!(
                !matches!(peer, Ok(PeerAddr::Name { .. })),
                "{bad} must never fall back to a DNS name"
            );
        }
    }

    #[test]
    fn peer_addresses_parse_without_resolution_and_serialize_as_text() {
        let onion = test_onion(3);
        let text = format!("{}:29610", onion.hostname());
        let parsed: PeerAddr = text.parse().unwrap();
        assert_eq!(parsed, PeerAddr::onion(onion, 29610));
        assert_eq!(parsed.to_string(), text);
        assert_eq!(
            PeerAddr::parse_with_default_port(&onion.hostname(), 29610).unwrap(),
            parsed
        );
        assert_eq!(
            "[::ffff:203.0.113.9]:29610".parse::<PeerAddr>().unwrap(),
            PeerAddr::Ip("203.0.113.9:29610".parse().unwrap())
        );
        assert_eq!(
            PeerAddr::parse_with_default_port("Seed.Example.org", 29610).unwrap(),
            PeerAddr::Name {
                host: "seed.example.org".into(),
                port: 29610
            }
        );
        for bad in [
            "",
            "1.2.3.4:0",
            "host:99999",
            "::1",
            "a b:1",
            "1.2.3:5",
            "x:",
        ] {
            assert!(bad.parse::<PeerAddr>().is_err(), "{bad}");
        }
        let legacy: Vec<PeerAddr> =
            serde_json::from_str(r#"["127.0.0.1:29410","[::1]:29411"]"#).unwrap();
        assert_eq!(legacy[0], PeerAddr::Ip("127.0.0.1:29410".parse().unwrap()));
        assert_eq!(
            serde_json::to_string(&legacy).unwrap(),
            r#"["127.0.0.1:29410","[::1]:29411"]"#
        );
    }

    #[test]
    fn wire_encoding_round_trips_and_rejects_bad_keys() {
        for address in [
            PeerAddr::Ip("203.0.113.1:29610".parse().unwrap()),
            PeerAddr::Ip("[2001:db8::1]:29610".parse().unwrap()),
            PeerAddr::onion(test_onion(1), 29610),
        ] {
            let wire = WireAddr::from_peer(&address).unwrap();
            assert_eq!(wire.to_peer().unwrap(), address);
        }
        assert!(
            WireAddr::from_peer(&PeerAddr::Name {
                host: "seed.example.org".into(),
                port: 1
            })
            .is_none()
        );
        let mut identity = [0u8; 32];
        identity[0] = 1;
        assert!(WireAddr::OnionV3([0; 32], 29610).to_peer().is_err());
        assert!(WireAddr::OnionV3(identity, 29610).to_peer().is_err());
        assert!(WireAddr::V4([1, 2, 3, 4], 0).to_peer().is_err());
    }
}
