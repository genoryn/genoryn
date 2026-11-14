//! Krypton coins on chain (`docs/spec/assets.md`): the coin part of version-3
//! transactions, the coin registry and its consensus rules.
//!
//! A version-3 transaction carries one *action* (the outer spend) and an [`AssetPart`]:
//!
//! * `transfer`, `issue`, `burn` move a coin: the outer spend is a coin spend (a
//!   version-2 proof with fee 0) and the GNR fee is paid by the inner [`FeeAction`], an
//!   ordinary GNR spend bound to the same sighash;
//! * `deploy`, `pause`, `unpause` move no coin: the outer spend is a GNR spend paying the
//!   fee itself (at least [`DEPLOY_FEE`] for a deploy).
//!
//! Issuer actions carry an ML-DSA-87 signature by the manifest's issuer key over the
//! sighash, which covers everything but the proofs and the signature itself.

use crate::codec::{Reader, Writer};
use crate::types::{HexHash, NOTE_CIPHERTEXT_LEN, Output};
use crate::{CoreError, Hash32, Result};
pub use krypton_assets::{AssetId, Backing, DEPLOY_FEE, GNR, Manifest, SIG_CTX, Supply};
use krypton_crypto::hash::tagged;
use std::collections::HashMap;

pub const MAX_MANIFEST_BYTES: usize = 8 * 1024;
/// The largest coin amount anything handles (the supply limit).
pub const MAX_COIN_UNITS: u64 = krypton_assets::MAX_SUPPLY;
pub const MAX_SIG_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum AssetOp {
    Transfer = 0,
    Issue = 1,
    Burn = 2,
    Deploy = 3,
    Pause = 4,
    Unpause = 5,
}

impl AssetOp {
    fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => Self::Transfer,
            1 => Self::Issue,
            2 => Self::Burn,
            3 => Self::Deploy,
            4 => Self::Pause,
            5 => Self::Unpause,
            _ => return Err(CoreError::Decode("unknown coin operation")),
        })
    }
    /// Operations whose outer spend moves the coin (and whose fee is a [`FeeAction`]).
    pub fn moves_coin(self) -> bool {
        matches!(self, Self::Transfer | Self::Issue | Self::Burn)
    }
    pub fn needs_issuer(self) -> bool {
        matches!(
            self,
            Self::Issue | Self::Deploy | Self::Pause | Self::Unpause
        )
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Issue => "issue",
            Self::Burn => "burn",
            Self::Deploy => "deploy",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
        }
    }
}

/// The GNR spend paying the fee of a coin movement (a version-1 statement with the
/// transaction's sighash, no mint and no lock).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FeeAction {
    #[serde(with = "hex::serde")]
    pub anchor: Hash32,
    pub nullifiers: [HexHash; 2],
    pub outputs: [Output; 2],
    pub fee: u64,
    #[serde(with = "hex::serde")]
    pub proof: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetPart {
    pub op: AssetOp,
    /// The public coin (`None`: a hidden transfer; deploys name no coin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coin: Option<AssetId>,
    /// Units destroyed (burns only).
    #[serde(default)]
    pub burn: u64,
    /// Canonical manifest text (deploys only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_hex")]
    pub issuer_sig: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_action: Option<FeeAction>,
}

fn flag(r: &mut Reader) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(CoreError::Decode("bad flag")),
    }
}

impl AssetPart {
    /// Encodes everything but the fee proof; `with_sig` is false for the sighash.
    pub(crate) fn encode(&self, w: &mut Writer, with_sig: bool) {
        w.u8(self.op as u8);
        match &self.coin {
            Some(c) => w.u8(1).raw(&c.to_bytes()),
            None => w.u8(0),
        };
        w.u64(self.burn);
        match &self.manifest {
            Some(m) => w.u8(1).bytes(m.as_bytes()),
            None => w.u8(0),
        };
        if with_sig {
            match &self.issuer_sig {
                Some(s) => w.u8(1).bytes(s),
                None => w.u8(0),
            };
        }
        match &self.fee_action {
            Some(f) => {
                w.u8(1).raw(&f.anchor);
                for n in &f.nullifiers {
                    w.raw(&n.0);
                }
                for o in &f.outputs {
                    w.raw(&o.cm).bytes(&o.ciphertext);
                }
                w.u64(f.fee)
            }
            None => w.u8(0),
        };
    }
    /// Decodes the part as written by `encode(.., true)`; the fee proof follows the
    /// outer proof and is filled in by the caller.
    pub(crate) fn decode(r: &mut Reader) -> Result<Self> {
        let op = AssetOp::from_u8(r.u8()?)?;
        let coin = if flag(r)? {
            Some(AssetId::from_bytes(&r.array::<17>()?).ok_or(CoreError::Decode("bad coin id"))?)
        } else {
            None
        };
        let burn = r.u64()?;
        let manifest = if flag(r)? {
            Some(
                String::from_utf8(r.bytes(MAX_MANIFEST_BYTES)?)
                    .map_err(|_| CoreError::Decode("manifest is not UTF-8"))?,
            )
        } else {
            None
        };
        let issuer_sig = if flag(r)? {
            Some(r.bytes(MAX_SIG_BYTES)?)
        } else {
            None
        };
        let fee_action = if flag(r)? {
            let anchor = r.array()?;
            let nullifiers = [HexHash(r.array()?), HexHash(r.array()?)];
            let mut out = || -> Result<Output> {
                Ok(Output {
                    cm: r.array()?,
                    ciphertext: r.bytes(NOTE_CIPHERTEXT_LEN)?,
                })
            };
            let outputs = [out()?, out()?];
            Some(FeeAction {
                anchor,
                nullifiers,
                outputs,
                fee: r.u64()?,
                proof: Vec::new(),
            })
        } else {
            None
        };
        Ok(Self {
            op,
            coin,
            burn,
            manifest,
            issuer_sig,
            fee_action,
        })
    }
}

/// What consensus keeps per coin.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetState {
    pub manifest: Manifest,
    /// Base units issued so far (all time).
    pub issued: u64,
    /// Base units burned so far.
    pub burned: u64,
    pub paused: bool,
    /// Height of the deploy.
    pub height: u64,
}

impl AssetState {
    pub fn outstanding(&self) -> u64 {
        self.issued.saturating_sub(self.burned)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(self.manifest.canonical().as_bytes())
            .u64(self.issued)
            .u64(self.burned)
            .u8(self.paused as u8)
            .u64(self.height);
        w.0
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut r = Reader::new(b);
        let text = String::from_utf8(r.bytes(MAX_MANIFEST_BYTES)?)
            .map_err(|_| CoreError::Decode("manifest is not UTF-8"))?;
        let manifest = Manifest::parse(&text).map_err(|_| CoreError::Decode("bad manifest"))?;
        let s = Self {
            manifest,
            issued: r.u64()?,
            burned: r.u64()?,
            paused: flag(&mut r)?,
            height: r.u64()?,
        };
        r.finish()?;
        Ok(s)
    }
}

/// Registry keys of a coin's ticker and name, which are unique on chain (first deploy
/// wins), so no later coin can pose as an earlier one.
pub fn name_keys(m: &Manifest) -> [String; 2] {
    [
        format!("t:{}", m.ticker),
        format!("n:{}", m.name.to_ascii_lowercase()),
    ]
}

/// Fingerprint of an issuer key, as pinned by [`crate::ChainParams::reserved_coins`].
pub fn issuer_fingerprint(key: &[u8]) -> Hash32 {
    tagged("Krypton/asset/issuer", key)
}

/// The coin registry as of the parent block.
pub trait AssetView {
    fn asset(&self, id: &AssetId) -> Option<AssetState>;
    /// The coin registered under a [`name_keys`] key.
    fn asset_named(&self, key: &str) -> Option<AssetId>;
}

/// Changes to the registry made by the transactions of one block (or one candidate
/// transaction), applied in order on top of an [`AssetView`].
#[derive(Default, Debug, Clone)]
pub struct AssetOverlay {
    states: HashMap<AssetId, AssetState>,
    names: HashMap<String, AssetId>,
    order: Vec<AssetId>,
}

impl AssetOverlay {
    fn get(&self, view: &impl AssetView, id: &AssetId) -> Option<AssetState> {
        self.states.get(id).cloned().or_else(|| view.asset(id))
    }
    fn named(&self, view: &impl AssetView, key: &str) -> Option<AssetId> {
        self.names
            .get(key)
            .copied()
            .or_else(|| view.asset_named(key))
    }
    fn put(&mut self, id: AssetId, s: AssetState) {
        if !self.states.contains_key(&id) {
            self.order.push(id);
        }
        self.states.insert(id, s);
    }
    /// Final state of every coin the block touched, in first-touch order.
    pub fn changes(&self) -> Vec<(AssetId, AssetState)> {
        self.order
            .iter()
            .map(|id| (*id, self.states[id].clone()))
            .collect()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

fn bad<T>(m: &'static str) -> Result<T> {
    Err(CoreError::Invalid(m))
}

/// Stateless rules of the coin part (the transaction is version 3).
pub fn check_structure(tx: &crate::Transaction) -> Result<()> {
    let Some(a) = &tx.asset else {
        return bad("version-3 transactions carry a coin part");
    };
    let none = |b: bool, m| if b { Ok(()) } else { bad(m) };
    none(
        a.op.moves_coin() == a.fee_action.is_some(),
        "coin movements pay their fee with a fee action, other operations directly",
    )?;
    none(
        a.op.needs_issuer() == a.issuer_sig.is_some(),
        "issuer signature present exactly for issuer operations",
    )?;
    none(
        (a.op == AssetOp::Deploy) == a.manifest.is_some(),
        "a manifest is carried by deploys only",
    )?;
    none(
        (a.op == AssetOp::Burn) == (a.burn > 0),
        "burns, and only burns, destroy units",
    )?;
    none(
        (a.op == AssetOp::Issue) == (tx.mint > 0),
        "issues, and only issues, create units",
    )?;
    none(
        tx.lock.is_none() || a.op == AssetOp::Transfer,
        "only coin transfers may spend a lock note",
    )?;
    none(
        a.burn <= krypton_zk::layout::MAX_VALUE && tx.mint <= krypton_zk::layout::MAX_VALUE,
        "amount out of range",
    )?;
    match a.op {
        AssetOp::Transfer => {}
        AssetOp::Issue | AssetOp::Burn | AssetOp::Pause | AssetOp::Unpause => {
            let Some(c) = &a.coin else {
                return bad("the operation names its coin");
            };
            none(!c.is_gnr(), "GNR is not a Krypton coin")?;
            if matches!(a.op, AssetOp::Pause | AssetOp::Unpause) {
                none(c.pausable, "the coin cannot be paused")?;
            }
        }
        AssetOp::Deploy => {
            none(a.coin.is_none(), "a deploy names no coin")?;
            none(tx.fee >= DEPLOY_FEE, "a deploy pays the deploy fee")?;
            let text = a.manifest.as_deref().unwrap_or_default();
            let m = Manifest::parse(text).map_err(|_| CoreError::Invalid("invalid manifest"))?;
            m.validate()
                .map_err(|_| CoreError::Invalid("invalid manifest"))?;
        }
    }
    if a.op.moves_coin() {
        none(tx.fee == 0, "coin spends pay no fee themselves")?;
        let f = a.fee_action.as_ref().expect("checked above");
        none(
            f.fee <= krypton_zk::layout::MAX_VALUE,
            "amount out of range",
        )?;
        none(
            f.outputs
                .iter()
                .all(|o| o.ciphertext.len() == NOTE_CIPHERTEXT_LEN),
            "wrong note ciphertext length",
        )?;
    }
    Ok(())
}

/// Stateful rules, applied in block order through `ov`. `sighash` is the transaction's
/// signed hash (`Transaction::sighash_bytes`).
pub fn check_state(
    p: &crate::ChainParams,
    view: &impl AssetView,
    ov: &mut AssetOverlay,
    tx: &crate::Transaction,
    height: u64,
) -> Result<()> {
    if height < p.assets_height {
        return bad("Krypton coins are not active yet");
    }
    let a = tx.asset.as_ref().expect("version 3");
    let sighash = tx.sighash_bytes();
    let signed = |m: &Manifest| {
        let sig = a.issuer_sig.as_deref().unwrap_or_default();
        if krypton_crypto::sign::mldsa_verify(&m.issuer, &sighash, sig, SIG_CTX) {
            Ok(())
        } else {
            bad("invalid issuer signature")
        }
    };
    if a.op == AssetOp::Deploy {
        let m = Manifest::parse(a.manifest.as_deref().unwrap_or_default())
            .map_err(|_| CoreError::Invalid("invalid manifest"))?;
        signed(&m)?;
        let id = m.id();
        if ov.get(view, &id).is_some() {
            return bad("coin already deployed");
        }
        let keys = name_keys(&m);
        if keys.iter().any(|k| ov.named(view, k).is_some()) {
            return bad("ticker or name already taken");
        }
        match p.reserved_coins.iter().find(|(t, _)| *t == m.ticker) {
            Some((_, fp)) if hex::encode(issuer_fingerprint(&m.issuer)) != *fp => {
                return bad("ticker reserved for another issuer");
            }
            _ => {}
        }
        for k in keys {
            ov.names.insert(k, id);
        }
        ov.put(
            id,
            AssetState {
                manifest: m,
                issued: 0,
                burned: 0,
                paused: false,
                height,
            },
        );
        return Ok(());
    }
    let Some(id) = a.coin else {
        // A hidden transfer: the circuit pins a non-pausable coin whose notes already
        // exist in the tree, so there is no registry rule to apply.
        return Ok(());
    };
    let Some(mut s) = ov.get(view, &id) else {
        return bad("unknown coin");
    };
    match a.op {
        AssetOp::Transfer => {
            if s.paused {
                return bad("the coin is paused");
            }
            return Ok(());
        }
        AssetOp::Issue => {
            signed(&s.manifest)?;
            if s.paused {
                return bad("the coin is paused");
            }
            let total = s
                .issued
                .checked_add(tx.mint)
                .ok_or(CoreError::Invalid("supply overflow"))?;
            let ok = match s.manifest.supply {
                Supply::Fixed => s.issued == 0 && Some(tx.mint) == s.manifest.cap,
                Supply::Mintable => {
                    total <= s.manifest.cap.unwrap_or(krypton_assets::MAX_SUPPLY)
                        && total <= krypton_assets::MAX_SUPPLY
                }
            };
            if !ok {
                return bad("the issue exceeds the declared supply");
            }
            s.issued = total;
        }
        AssetOp::Burn => {
            if !s.manifest.burn {
                return bad("the coin does not allow burning");
            }
            if s.paused {
                return bad("the coin is paused");
            }
            s.burned = s
                .burned
                .checked_add(a.burn)
                .filter(|b| *b <= s.issued)
                .ok_or(CoreError::Invalid("burn exceeds the supply"))?;
        }
        AssetOp::Pause | AssetOp::Unpause => {
            signed(&s.manifest)?;
            if !s.manifest.pause {
                return bad("the coin cannot be paused");
            }
            let pause = a.op == AssetOp::Pause;
            if s.paused == pause {
                return bad("the coin is already in that state");
            }
            s.paused = pause;
        }
        AssetOp::Deploy => unreachable!(),
    }
    ov.put(id, s);
    Ok(())
}

mod opt_hex {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_some(&hex::encode(b)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|h| hex::decode(h).map_err(serde::de::Error::custom))
            .transpose()
    }
}
