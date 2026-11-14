//! Krypton coins: user-issued, always-shielded coins on Genoryn Krypton
//! (`docs/spec/assets.md`).
//!
//! A coin is defined by its manifest (Krypton Asset Manifest, KAM v1): a short declarative
//! document with a single canonical encoding. Everything an issuer may ever do is declared
//! in it, it is hashed into the coin's ID, and the risk labels wallets show are generated
//! from it, so no power can be hidden or added later.

#![forbid(unsafe_code)]

use krypton_crypto::field::MODULUS;
use krypton_crypto::hash::{Hash32, tagged};
use std::fmt;

/// GNR paid (to the miners' reward pool) to deploy a coin: 10 GNR.
pub const DEPLOY_FEE: u64 = 10 * 100_000_000;
/// The largest supply a coin may ever have, in base units (the range proofs' limit).
pub const MAX_SUPPLY: u64 = (1 << 60) - 1;
/// Context string of issuer signatures (ML-DSA-87).
pub const SIG_CTX: &[u8] = b"Krypton/asset/v1";
pub const MAX_DECIMALS: u8 = 8;
/// Tickers consensus refuses: the network's own coin and Bitcoin.
pub const RESERVED_TICKERS: &[&str] = &["GNRK", "BTCK", "XBTK", "KRK", "KRYPTONK", "GENORYNK"];
/// Words no coin name may contain (any case).
pub const RESERVED_NAME_WORDS: &[&str] = &["genoryn", "bitcoin"];
/// Tickers of major coins: wallets mark coins using them as possible impersonations
/// unless verified (this is not a consensus rule).
pub const MAJOR_TICKERS: &[&str] = &[
    "ETHK", "SOLK", "USDTK", "USDCK", "BNBK", "XRPK", "ADAK", "DOGEK", "TRXK", "TONK", "LTCK",
    "XMRK", "ZECK", "DOTK", "AVAXK", "LINKK", "DAIK", "MATICK", "POLK", "SHIBK", "BCHK", "XLMK",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssetError {
    #[error("manifest: {0}")]
    Invalid(String),
}

fn bad<T>(m: impl Into<String>) -> Result<T, AssetError> {
    Err(AssetError::Invalid(m.into()))
}

/// How much of a coin may exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Supply {
    /// Issued once, exactly `cap` base units.
    Fixed,
    /// The issuer may issue more, up to `cap` if there is one.
    Mintable,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backing {
    None,
    IssuerReserves,
    Bridge,
}

/// A coin's manifest (KAM v1).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub name: String,
    pub ticker: String,
    pub decimals: u8,
    pub supply: Supply,
    /// Base units; `None` means no cap (mintable only).
    pub cap: Option<u64>,
    /// Holders may burn their coins.
    pub burn: bool,
    /// The issuer may pause all transfers.
    pub pause: bool,
    pub backing: Backing,
    /// Bridges: the original chain and coin, e.g. `solana:SOL`.
    pub origin: Option<String>,
    pub reserves: Option<String>,
    pub website: Option<String>,
    pub terms: Option<String>,
    /// SHA-256 of the logo file.
    #[serde(default, with = "opt_hex32")]
    pub logo: Option<[u8; 32]>,
    /// ML-DSA-87 public key of the issuer.
    #[serde(with = "hex::serde")]
    pub issuer: Vec<u8>,
}

const KEYS: [&str; 15] = [
    "kam", "name", "ticker", "decimals", "supply", "cap", "burn", "pause", "backing", "origin",
    "reserves", "website", "terms", "logo", "issuer",
];
/// Keys that may be left out (all others are required).
const OPTIONAL: [&str; 5] = ["origin", "reserves", "website", "terms", "logo"];
/// Length of an ML-DSA-87 public key.
pub const ISSUER_KEY_LEN: usize = 2592;

impl Manifest {
    /// The canonical text: the only encoding consensus accepts.
    pub fn canonical(&self) -> String {
        let mut s = String::new();
        let mut line = |k: &str, v: &str| {
            s.push_str(k);
            s.push_str(": ");
            s.push_str(v);
            s.push('\n');
        };
        line("kam", "1");
        line("name", &self.name);
        line("ticker", &self.ticker);
        line("decimals", &self.decimals.to_string());
        line(
            "supply",
            match self.supply {
                Supply::Fixed => "fixed",
                Supply::Mintable => "mintable",
            },
        );
        line("cap", &self.cap.map_or("none".into(), |c| c.to_string()));
        line("burn", if self.burn { "holders" } else { "none" });
        line("pause", if self.pause { "yes" } else { "no" });
        line(
            "backing",
            match self.backing {
                Backing::None => "none",
                Backing::IssuerReserves => "issuer-reserves",
                Backing::Bridge => "bridge",
            },
        );
        for (k, v) in [
            ("origin", &self.origin),
            ("reserves", &self.reserves),
            ("website", &self.website),
            ("terms", &self.terms),
        ] {
            if let Some(v) = v {
                line(k, v);
            }
        }
        if let Some(l) = &self.logo {
            line("logo", &hex::encode(l));
        }
        line("issuer", &hex::encode(&self.issuer));
        s
    }

    /// Parses a manifest; refuses anything that is not exactly its canonical encoding.
    pub fn parse(text: &str) -> Result<Self, AssetError> {
        if text.len() > 8 * 1024 {
            return bad("too long");
        }
        let mut fields: Vec<(&str, &str)> = Vec::new();
        for l in text.split_terminator('\n') {
            let Some((k, v)) = l.split_once(": ") else {
                return bad(format!("not a `key: value` line: {l:.40}"));
            };
            fields.push((k, v));
        }
        let mut order = KEYS.iter().filter(|k| fields.iter().any(|(f, _)| f == *k));
        for (k, _) in &fields {
            if order.next() != Some(k) {
                return bad(format!("unknown, repeated or misplaced field `{k}`"));
            }
        }
        let get = |k: &str| fields.iter().find(|(f, _)| *f == k).map(|(_, v)| *v);
        for k in KEYS {
            if !OPTIONAL.contains(&k) && get(k).is_none() {
                return bad(format!("missing field `{k}`"));
            }
        }
        if get("kam") != Some("1") {
            return bad("unsupported manifest version");
        }
        let req = |k: &str| get(k).unwrap_or_default().to_string();
        let opt = |k: &str| get(k).map(str::to_string);
        let num = |k: &str| -> Result<u64, AssetError> {
            let v = req(k);
            if v.is_empty()
                || (v.len() > 1 && v.starts_with('0'))
                || !v.bytes().all(|b| b.is_ascii_digit())
            {
                return bad(format!("`{k}` must be a whole number"));
            }
            v.parse().or_else(|_| bad(format!("`{k}` is too large")))
        };
        let decimals =
            u8::try_from(num("decimals")?).or_else(|_| bad("`decimals` is too large"))?;
        let supply = match req("supply").as_str() {
            "fixed" => Supply::Fixed,
            "mintable" => Supply::Mintable,
            _ => return bad("`supply` must be fixed or mintable"),
        };
        let cap = if get("cap") == Some("none") {
            None
        } else {
            Some(num("cap")?)
        };
        let flag = |k: &str, yes: &str, no: &str| -> Result<bool, AssetError> {
            match get(k) {
                Some(v) if v == yes => Ok(true),
                Some(v) if v == no => Ok(false),
                _ => bad(format!("`{k}` must be {yes} or {no}")),
            }
        };
        let backing = match req("backing").as_str() {
            "none" => Backing::None,
            "issuer-reserves" => Backing::IssuerReserves,
            "bridge" => Backing::Bridge,
            _ => return bad("`backing` must be none, issuer-reserves or bridge"),
        };
        let logo = match get("logo") {
            None => None,
            Some(h) => Some(
                hex::decode(h)
                    .ok()
                    .and_then(|b| <[u8; 32]>::try_from(b).ok())
                    .ok_or_else(|| AssetError::Invalid("`logo` must be a SHA-256 in hex".into()))?,
            ),
        };
        let issuer = hex::decode(req("issuer")).or_else(|_| bad("`issuer` must be hex"))?;
        let m = Manifest {
            name: req("name"),
            ticker: req("ticker"),
            decimals,
            supply,
            cap,
            burn: flag("burn", "holders", "none")?,
            pause: flag("pause", "yes", "no")?,
            backing,
            origin: opt("origin"),
            reserves: opt("reserves"),
            website: opt("website"),
            terms: opt("terms"),
            logo,
            issuer,
        };
        m.validate()?;
        if m.canonical() != text {
            return bad("not in canonical form");
        }
        Ok(m)
    }

    /// The consensus rules every manifest must satisfy.
    pub fn validate(&self) -> Result<(), AssetError> {
        let name = &self.name;
        if !(3..=40).contains(&name.len())
            || !name.bytes().all(|b| (0x20..0x7f).contains(&b))
            || name.starts_with(' ')
            || name.contains("  ")
        {
            return bad("the name must be 3 to 40 printable ASCII characters");
        }
        if !name.ends_with(" Krypton") || name.len() == " Krypton".len() {
            return bad("the name must end with \" Krypton\" (e.g. \"Nova Krypton\")");
        }
        let lower = name.to_ascii_lowercase();
        if RESERVED_NAME_WORDS.iter().any(|w| lower.contains(w)) {
            return bad("this name is reserved");
        }
        let t = &self.ticker;
        if !(2..=9).contains(&t.len())
            || !t
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            || !t.ends_with('K')
        {
            return bad("the ticker must be 2 to 9 characters A-Z 0-9 ending with K (e.g. NOVAK)");
        }
        if RESERVED_TICKERS.contains(&t.as_str()) {
            return bad("this ticker is reserved");
        }
        if self.decimals > MAX_DECIMALS {
            return bad("at most 8 decimals");
        }
        match (self.supply, self.cap) {
            (Supply::Fixed, None) => return bad("a fixed supply needs a cap"),
            (_, Some(0)) => return bad("the cap must be positive"),
            (_, Some(c)) if c > MAX_SUPPLY => return bad("the cap is above 2^60 - 1 base units"),
            _ => {}
        }
        let url = |u: &Option<String>| {
            u.as_ref().is_none_or(|u| {
                u.starts_with("https://")
                    && u.len() <= 200
                    && u.bytes().all(|b| (0x21..0x7f).contains(&b))
            })
        };
        if !(url(&self.reserves) && url(&self.website) && url(&self.terms)) {
            return bad("links must be https:// URLs of at most 200 characters");
        }
        match self.backing {
            Backing::IssuerReserves if self.reserves.is_none() => {
                return bad("issuer-reserves backing needs a `reserves` link");
            }
            Backing::Bridge if self.reserves.is_none() || self.origin.is_none() => {
                return bad("a bridge needs `origin` and `reserves`");
            }
            _ => {}
        }
        if let Some(o) = &self.origin {
            if self.backing != Backing::Bridge {
                return bad("`origin` is only for bridges");
            }
            if !(3..=40).contains(&o.len())
                || !o.contains(':')
                || !o
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-' || b == b'.')
            {
                return bad("`origin` must look like chain:COIN");
            }
        }
        if self.issuer.len() != ISSUER_KEY_LEN {
            return bad("the issuer must be an ML-DSA-87 public key");
        }
        Ok(())
    }

    pub fn id(&self) -> AssetId {
        AssetId::derive(
            &tagged("Krypton/asset/id", self.canonical().as_bytes()),
            self.pause,
        )
    }

    /// Plain-language risk labels, in the order wallets show them.
    pub fn risk_labels(&self, verified: bool) -> Vec<String> {
        let mut v = Vec::new();
        let units = |n: u64| format_units(n, self.decimals);
        v.push(match (self.supply, self.cap) {
            (Supply::Fixed, Some(c)) => format!(
                "Fixed supply: {} {}, nobody can ever create more",
                units(c),
                self.ticker
            ),
            (Supply::Mintable, Some(c)) => format!(
                "The issuer can create more, up to {} {} in total",
                units(c),
                self.ticker
            ),
            _ => "The issuer can create any amount at any time".into(),
        });
        if self.burn {
            v.push(
                "Holders can burn (destroy) their coins, e.g. to redeem them with the issuer"
                    .into(),
            );
        }
        if self.pause {
            v.push("The issuer can stop all transfers of this coin at any time; its transfers show which coin moves".into());
        }
        v.push(
            match self.backing {
                Backing::IssuerReserves => "Its value depends on the issuer actually holding the reserves and honouring redemptions",
                Backing::Bridge => "Custodial bridge: its value depends on the custodian holding the original coins",
                Backing::None => "Not backed by anything: its value is only what people pay for it",
            }
            .into(),
        );
        if !verified {
            v.push("Not verified by the Genoryn Krypton project: anyone can deploy a coin with any name".into());
            if MAJOR_TICKERS.contains(&self.ticker.as_str()) {
                v.push(format!(
                    "{} is the ticker of a major coin: this may be an impersonation",
                    self.ticker
                ));
            }
        }
        v
    }
}

/// Formats base units with the coin's decimals: 1500000 with 6 decimals is "1.5".
pub fn format_units(v: u64, decimals: u8) -> String {
    if decimals == 0 {
        return group(v);
    }
    let d = 10u64.pow(decimals as u32);
    let frac = format!("{:0w$}", v % d, w = decimals as usize);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        group(v / d)
    } else {
        format!("{}.{frac}", group(v / d))
    }
}

/// Parses "1.5" into base units with the coin's decimals.
pub fn parse_units(s: &str, decimals: u8) -> Option<u64> {
    let s = s.trim().replace(',', "");
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    if int.is_empty() && frac.is_empty()
        || frac.len() > decimals as usize
        || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let d = 10u64.checked_pow(decimals as u32)?;
    let i: u64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let f: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<w$}", w = decimals as usize).parse().ok()?
    };
    i.checked_mul(d)?
        .checked_add(f)
        .filter(|v| *v <= MAX_SUPPLY)
}

fn group(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A coin as proofs see it: two field elements and the pause flag. GNR is `(0, 0, false)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AssetId {
    pub a0: u64,
    pub a1: u64,
    pub pausable: bool,
}

pub const GNR: AssetId = AssetId {
    a0: 0,
    a1: 0,
    pausable: false,
};

impl AssetId {
    fn derive(h: &Hash32, pausable: bool) -> Self {
        let w = |i: usize| u64::from_le_bytes(h[i * 8..i * 8 + 8].try_into().expect("8")) % MODULUS;
        Self {
            a0: w(0),
            a1: w(1),
            pausable,
        }
    }
    pub fn is_gnr(&self) -> bool {
        *self == GNR
    }
    /// The three field elements proofs commit to.
    pub fn elements(&self) -> [u64; 3] {
        [self.a0, self.a1, self.pausable as u64]
    }
    pub fn to_bytes(&self) -> [u8; 17] {
        let mut b = [0u8; 17];
        b[..8].copy_from_slice(&self.a0.to_le_bytes());
        b[8..16].copy_from_slice(&self.a1.to_le_bytes());
        b[16] = self.pausable as u8;
        b
    }
    /// Strict decoding: field elements below the modulus, flag 0 or 1.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let b: &[u8; 17] = b.try_into().ok()?;
        let a0 = u64::from_le_bytes(b[..8].try_into().ok()?);
        let a1 = u64::from_le_bytes(b[8..16].try_into().ok()?);
        if a0 >= MODULUS || a1 >= MODULUS || b[16] > 1 {
            return None;
        }
        Some(Self {
            a0,
            a1,
            pausable: b[16] == 1,
        })
    }
    pub fn to_hex(&self) -> String {
        hex::encode(self.to_bytes())
    }
    pub fn from_hex(s: &str) -> Option<Self> {
        Self::from_bytes(&hex::decode(s).ok()?)
    }
    /// A short fingerprint for people to compare (first 8 hex characters of the ID).
    pub fn fingerprint(&self) -> String {
        self.to_hex()[..8].to_string()
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl serde::Serialize for AssetId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for AssetId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        Self::from_hex(&s).ok_or_else(|| serde::de::Error::custom("bad coin id"))
    }
}

mod opt_hex32 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_some(&hex::encode(b)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let v = Option::<String>::deserialize(d)?;
        v.map(|s| {
            hex::decode(s)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .ok_or_else(|| serde::de::Error::custom("bad hash"))
        })
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usdk() -> Manifest {
        Manifest {
            name: "Dollar Krypton".into(),
            ticker: "USDK".into(),
            decimals: 6,
            supply: Supply::Mintable,
            cap: None,
            burn: true,
            pause: false,
            backing: Backing::IssuerReserves,
            origin: None,
            reserves: Some("https://genoryn.io/coins/usdk/reserves".into()),
            website: Some("https://genoryn.io/coins/usdk".into()),
            terms: None,
            logo: Some([7; 32]),
            issuer: vec![1; ISSUER_KEY_LEN],
        }
    }

    #[test]
    fn canonical_roundtrip_and_strictness() {
        let m = usdk();
        let text = m.canonical();
        assert_eq!(Manifest::parse(&text).unwrap(), m);
        assert!(
            Manifest::parse(&text.replace('\n', "\r\n")).is_err(),
            "CRLF"
        );
        assert!(
            Manifest::parse(&text.replace("decimals: 6", "decimals: 06")).is_err(),
            "leading zero"
        );
        assert!(
            Manifest::parse(&format!("{text}extra: 1\n")).is_err(),
            "unknown field"
        );
        let swapped = text.replace(
            "name: Dollar Krypton\nticker: USDK",
            "ticker: USDK\nname: Dollar Krypton",
        );
        assert!(Manifest::parse(&swapped).is_err(), "field order");
        assert!(
            Manifest::parse(&text.replace("burn: holders\n", "")).is_err(),
            "missing field"
        );
        assert!(
            Manifest::parse(text.trim_end()).is_err(),
            "no final newline"
        );
    }

    #[test]
    fn names_tickers_and_reserved_words() {
        let ok = |f: &dyn Fn(&mut Manifest)| {
            let mut m = usdk();
            f(&mut m);
            m.validate().is_ok()
        };
        assert!(ok(&|_| {}));
        assert!(
            !ok(&|m| m.name = "Dollar".into()),
            "name must end with Krypton"
        );
        assert!(!ok(&|m| m.name = " Krypton".into()));
        assert!(!ok(&|m| m.name = "Bitcoin Krypton".into()), "reserved word");
        assert!(
            !ok(&|m| m.name = "GENORYN Krypton".into()),
            "reserved word, any case"
        );
        assert!(!ok(&|m| m.ticker = "USD".into()), "ticker must end with K");
        assert!(!ok(&|m| m.ticker = "usdk".into()), "upper case");
        assert!(!ok(&|m| m.ticker = "BTCK".into()), "reserved ticker");
        assert!(!ok(&|m| m.ticker = "GNRK".into()), "reserved ticker");
        assert!(
            ok(&|m| m.ticker = "SOLK".into()),
            "majors are a wallet warning, not consensus"
        );
        assert!(!ok(&|m| m.decimals = 9));
        assert!(!ok(&|m| {
            m.supply = Supply::Fixed;
            m.cap = None
        }));
        assert!(!ok(&|m| m.cap = Some(MAX_SUPPLY + 1)));
        assert!(!ok(&|m| m.reserves = None), "reserves backing needs a link");
        assert!(
            !ok(&|m| m.website = Some("http://x.example".into())),
            "https only"
        );
        assert!(!ok(&|m| m.issuer = vec![1; 10]));
        assert!(
            !ok(&|m| m.origin = Some("solana:SOL".into())),
            "origin only for bridges"
        );
        assert!(ok(&|m| {
            m.backing = Backing::Bridge;
            m.origin = Some("solana:SOL".into())
        }));
    }

    #[test]
    fn ids_depend_on_everything_and_carry_the_pause_flag() {
        let a = usdk();
        let mut b = usdk();
        b.decimals = 2;
        let mut c = usdk();
        c.pause = true;
        assert_ne!(a.id(), b.id());
        assert_ne!(a.id(), c.id());
        assert!(!a.id().pausable && c.id().pausable);
        assert!(!a.id().is_gnr());
        let id = a.id();
        assert_eq!(AssetId::from_bytes(&id.to_bytes()), Some(id));
        assert_eq!(AssetId::from_hex(&id.to_hex()), Some(id));
        let mut bytes = id.to_bytes();
        bytes[16] = 2;
        assert_eq!(AssetId::from_bytes(&bytes), None, "flag must be 0 or 1");
        bytes[16] = 0;
        bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            AssetId::from_bytes(&bytes),
            None,
            "non-canonical field element"
        );
    }

    #[test]
    fn risk_labels_say_everything() {
        let l = usdk().risk_labels(false);
        assert!(l[0].contains("any amount"));
        assert!(l.iter().any(|x| x.contains("burn")));
        assert!(l.iter().any(|x| x.contains("reserves")));
        assert!(l.iter().any(|x| x.contains("Not verified")));
        assert!(
            !usdk()
                .risk_labels(true)
                .iter()
                .any(|x| x.contains("Not verified"))
        );
        let mut f = usdk();
        f.supply = Supply::Fixed;
        f.cap = Some(21_000_000_000_000);
        f.burn = false;
        f.pause = true;
        f.backing = Backing::None;
        f.reserves = None;
        let l = f.risk_labels(true);
        assert_eq!(
            l[0],
            "Fixed supply: 21,000,000 USDK, nobody can ever create more"
        );
        assert!(l.iter().any(|x| x.contains("stop all transfers")));
        let mut s = usdk();
        s.ticker = "SOLK".into();
        assert!(
            s.risk_labels(false)
                .iter()
                .any(|x| x.contains("impersonation"))
        );
    }

    #[test]
    fn units() {
        assert_eq!(format_units(1_500_000, 6), "1.5");
        assert_eq!(format_units(1_000_000_000, 6), "1,000");
        assert_eq!(format_units(7, 0), "7");
        assert_eq!(parse_units("1.5", 6), Some(1_500_000));
        assert_eq!(parse_units("1,000", 6), Some(1_000_000_000));
        assert_eq!(parse_units("0.0000001", 6), None, "too many decimals");
        assert_eq!(parse_units("abc", 6), None);
        assert_eq!(parse_units("", 6), None);
    }
}
