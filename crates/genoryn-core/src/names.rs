//! On-chain usernames v1.
//!
//! A registration is an ordinary private transfer with one extra output: the price
//! of the name, paid to the network's burn address, with a signed name record in that
//! output's memo. The burn address derives from a hash-to-curve spend key nobody can
//! know, so the paid value can never be spent; its viewing key is public, so every
//! node decrypts the output, checks the commitment-bound value and reads the record.
//!
//! Names are derived state: they never make a block invalid and are not committed in
//! block headers, so nodes without this module accept the same chain. Every node that
//! has it builds the same table by applying valid operations in chain order, and a
//! reorganization rebuilds it with the rest of the state. Payments stay private: a
//! name publishes only its receiving address, never who pays it or how much.
use crate::{
    codec::{self, Hash},
    params::Network,
    privacy::ShieldedTx,
};
use anyhow::{Result, ensure};
use ff::{FromUniformBytes, PrimeField};
use group::{Curve, GroupEncoding};
use orchard::{
    Address,
    keys::{FullViewingKey, IncomingViewingKey, Scope},
};
use pasta_curves::{arithmetic::CurveExt, pallas};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::{collections::BTreeMap, sync::OnceLock};

const MAGIC: &[u8; 8] = b"GNRNAME\x01";
pub const OP_REGISTER: u8 = 1;
pub const OP_UPDATE: u8 = 2;
pub const GNR: u64 = 1_000_000_000_000;
/// Changing the receiver of an owned name.
pub const UPDATE_PRICE: u64 = GNR;
const RESERVED: [&str; 6] = [
    "admin",
    "administrator",
    "genoryn",
    "support",
    "security",
    "treasury",
];

/// 3-32 lowercase ASCII letters, digits or underscores, starting with a letter.
/// A leading `@` and upper case are accepted and normalized.
pub fn canonical_name(text: &str) -> Result<String> {
    let name = text
        .trim()
        .strip_prefix('@')
        .unwrap_or(text.trim())
        .to_ascii_lowercase();
    ensure!(
        (3..=32).contains(&name.len())
            && name.as_bytes()[0].is_ascii_lowercase()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "a username has 3-32 letters, digits or underscores and starts with a letter"
    );
    ensure!(
        !RESERVED.contains(&name.as_str()),
        "this username is reserved"
    );
    Ok(name)
}

/// The amount burned to register `name` forever: shorter names cost more.
pub fn registration_price(name: &str) -> u64 {
    match name.len() {
        0..=4 => 1_000 * GNR,
        5..=6 => 100 * GNR,
        _ => 10 * GNR,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRecord {
    pub op: u8,
    pub name: String,
    pub receiver: [u8; 43],
    pub owner: [u8; 32],
    pub sequence: u32,
    pub signature: [u8; 64],
}
impl NameRecord {
    /// The bytes the owner key signs; bound to this network's genesis.
    pub fn signing_message(&self, network: Network) -> Vec<u8> {
        let mut message = b"GENORYN/name-record/v1".to_vec();
        message.extend_from_slice(&network.genesis());
        message.push(self.op);
        message.push(self.name.len() as u8);
        message.extend_from_slice(self.name.as_bytes());
        message.extend_from_slice(&self.receiver);
        message.extend_from_slice(&self.owner);
        message.extend_from_slice(&self.sequence.to_le_bytes());
        message
    }
    pub fn signature_valid(&self, network: Network) -> bool {
        let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&self.owner) else {
            return false;
        };
        key.verify_strict(
            &self.signing_message(network),
            &ed25519_dalek::Signature::from_bytes(&self.signature),
        )
        .is_ok()
    }
    pub fn encode(&self) -> [u8; 512] {
        let mut memo = [0u8; 512];
        let mut at = 0;
        let mut put = |bytes: &[u8]| {
            memo[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        };
        put(MAGIC);
        put(&[self.op, self.name.len() as u8]);
        put(self.name.as_bytes());
        put(&self.receiver);
        put(&self.owner);
        put(&self.sequence.to_le_bytes());
        put(&self.signature);
        memo
    }
    /// Strict parse: magic, a canonical name, known operation and zero padding.
    pub fn decode(memo: &[u8; 512]) -> Option<Self> {
        if &memo[..8] != MAGIC {
            return None;
        }
        let op = memo[8];
        let length = memo[9] as usize;
        if !matches!(op, OP_REGISTER | OP_UPDATE) || !(3..=32).contains(&length) {
            return None;
        }
        let name = std::str::from_utf8(&memo[10..10 + length]).ok()?.to_owned();
        if canonical_name(&name).ok()? != name {
            return None;
        }
        let mut at = 10 + length;
        let mut take = |size: usize| {
            let bytes = &memo[at..at + size];
            at += size;
            bytes
        };
        let receiver = take(43).try_into().ok()?;
        let owner = take(32).try_into().ok()?;
        let sequence = u32::from_le_bytes(take(4).try_into().ok()?);
        let signature = take(64).try_into().ok()?;
        if memo[at..].iter().any(|byte| *byte != 0) {
            return None;
        }
        Some(Self {
            op,
            name,
            receiver,
            owner,
            sequence,
            signature,
        })
    }
}

fn uniform<F: FromUniformBytes<64>>(label: &[u8], network: Network, counter: u32) -> F {
    let mut digest = Sha512::new();
    digest.update(b"GENORYN/name-burn/v1/");
    digest.update(label);
    digest.update([network.id()]);
    digest.update(counter.to_le_bytes());
    F::from_uniform_bytes(&digest.finalize().into())
}
fn derive_burn_key(network: Network) -> FullViewingKey {
    for counter in 0u32.. {
        // A hash-to-curve point has no known discrete logarithm, so no spend
        // authorizing key exists for it; the sign is normalized as Orchard requires.
        let point = pallas::Point::hash_to_curve("GENORYN-name-burn-v1")(
            &[&[network.id()][..], &counter.to_le_bytes()].concat(),
        );
        let mut ak = point.to_affine().to_bytes();
        if ak[31] & 0x80 != 0 {
            ak = (-point).to_affine().to_bytes();
        }
        let nk = uniform::<pallas::Base>(b"nk", network, counter).to_repr();
        let rivk = uniform::<pallas::Scalar>(b"rivk", network, counter).to_repr();
        let mut bytes = [0u8; 96];
        bytes[..32].copy_from_slice(&ak);
        bytes[32..64].copy_from_slice(&nk);
        bytes[64..].copy_from_slice(&rivk);
        if let Some(fvk) = FullViewingKey::from_bytes(&bytes) {
            return fvk;
        }
    }
    unreachable!("a valid burn key is found within a few attempts")
}
/// The public viewing key of the burn address; it has no spending key.
pub fn burn_viewing_key(network: Network) -> &'static FullViewingKey {
    static KEYS: [OnceLock<FullViewingKey>; 3] =
        [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    KEYS[network.id() as usize % 3].get_or_init(|| derive_burn_key(network))
}
pub fn burn_address(network: Network) -> Address {
    burn_viewing_key(network).address_at(0u32, Scope::External)
}
fn burn_ivk(network: Network) -> IncomingViewingKey {
    burn_viewing_key(network).to_ivk(Scope::External)
}
/// Whether `address` is a burn address of `network` (any diversifier).
pub fn is_burn_address(network: Network, address: &Address) -> bool {
    burn_ivk(network).diversifier_index(address).is_some()
}

/// A name operation found in a transaction, before chain-order rules apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameOperation {
    pub record: NameRecord,
    pub burned: u64,
    pub txid: Hash,
    pub action: u32,
}
/// Name records paid to the burn address in `tx`. Decryption checks each note
/// against its on-chain commitment, so `burned` is the value actually paid.
pub fn operations(network: Network, tx: &ShieldedTx) -> Vec<NameOperation> {
    if tx.context.kind != 0 {
        return Vec::new();
    }
    let (Ok(bundle), Ok(txid)) = (tx.bundle.to_bundle(), tx.id()) else {
        return Vec::new();
    };
    bundle
        .decrypt_outputs_with_keys(&[burn_ivk(network)])
        .into_iter()
        .filter_map(|(action, _, note, _, memo)| {
            Some(NameOperation {
                record: NameRecord::decode(&memo)?,
                burned: note.value().inner(),
                txid,
                action: action as u32,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameEntry {
    pub name: String,
    #[serde(with = "hex_bytes")]
    pub receiver: [u8; 43],
    #[serde(with = "hex_bytes")]
    pub owner: [u8; 32],
    pub sequence: u32,
    pub registered_height: u64,
    pub updated_height: u64,
    #[serde(with = "hex_bytes")]
    pub txid: Hash,
    pub burned: u64,
}
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer, const N: usize>(
        bytes: &[u8; N],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[u8; N], D::Error> {
        hex::decode(String::deserialize(d)?)
            .map_err(D::Error::custom)?
            .try_into()
            .map_err(|_| D::Error::custom("wrong length"))
    }
}

/// Applies one operation in chain order. A rejected operation changes nothing; its
/// burned value stays burned, which is why wallets check availability first.
pub fn apply(
    names: &mut BTreeMap<String, NameEntry>,
    network: Network,
    operation: &NameOperation,
    height: u64,
) -> std::result::Result<(), &'static str> {
    let record = &operation.record;
    if !record.signature_valid(network) {
        return Err("invalid owner signature");
    }
    if Option::<Address>::from(Address::from_raw_address_bytes(&record.receiver)).is_none() {
        return Err("invalid receiver");
    }
    match record.op {
        OP_REGISTER => {
            if names.contains_key(&record.name) {
                return Err("name already registered");
            }
            if record.sequence != 0 {
                return Err("registration sequence must be zero");
            }
            if operation.burned < registration_price(&record.name) {
                return Err("burned amount below the name price");
            }
            names.insert(
                record.name.clone(),
                NameEntry {
                    name: record.name.clone(),
                    receiver: record.receiver,
                    owner: record.owner,
                    sequence: 0,
                    registered_height: height,
                    updated_height: height,
                    txid: operation.txid,
                    burned: operation.burned,
                },
            );
        }
        OP_UPDATE => {
            let Some(entry) = names.get_mut(&record.name) else {
                return Err("name not registered");
            };
            if entry.owner != record.owner {
                return Err("name belongs to another owner");
            }
            if record.sequence != entry.sequence + 1 {
                return Err("update sequence must follow the current one");
            }
            if operation.burned < UPDATE_PRICE {
                return Err("burned amount below the update price");
            }
            entry.receiver = record.receiver;
            entry.sequence = record.sequence;
            entry.updated_height = height;
            entry.burned = entry.burned.saturating_add(operation.burned);
        }
        _ => return Err("unknown name operation"),
    }
    Ok(())
}

/// Stable identifier of the names table, for cross-node comparisons in tests.
pub fn table_digest(names: &BTreeMap<String, NameEntry>) -> Result<Hash> {
    Ok(codec::hash(
        b"GENORYN/names/v1",
        &serde_json::to_vec(names)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn receiver() -> [u8; 43] {
        let sk = orchard::keys::SpendingKey::from_bytes([7; 32]).unwrap();
        FullViewingKey::from(&sk)
            .address_at(0u32, Scope::External)
            .to_raw_address_bytes()
    }
    fn signed(op: u8, name: &str, sequence: u32, key: &SigningKey) -> NameRecord {
        let mut record = NameRecord {
            op,
            name: name.into(),
            receiver: receiver(),
            owner: key.verifying_key().to_bytes(),
            sequence,
            signature: [0; 64],
        };
        record.signature = key
            .sign(&record.signing_message(Network::Devnet))
            .to_bytes();
        record
    }
    fn operation(record: NameRecord, burned: u64) -> NameOperation {
        NameOperation {
            record,
            burned,
            txid: [9; 32],
            action: 0,
        }
    }

    #[test]
    fn names_are_canonical_and_reserved_names_are_refused() {
        assert_eq!(canonical_name("@Max_01").unwrap(), "max_01");
        let long = "a".repeat(33);
        for bad in [
            "ab",
            "1max",
            "max-1",
            "ma x",
            long.as_str(),
            "@genoryn",
            "Treasury",
        ] {
            assert!(canonical_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn prices_rise_for_short_names() {
        assert_eq!(registration_price("max"), 1_000 * GNR);
        assert_eq!(registration_price("maxi"), 1_000 * GNR);
        assert_eq!(registration_price("maxim"), 100 * GNR);
        assert_eq!(registration_price("maxima"), 100 * GNR);
        assert_eq!(registration_price("maximus"), 10 * GNR);
    }

    #[test]
    fn records_round_trip_and_reject_noise() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let record = signed(OP_REGISTER, "maximus", 0, &key);
        let memo = record.encode();
        assert_eq!(NameRecord::decode(&memo), Some(record.clone()));
        assert!(record.signature_valid(Network::Devnet));
        assert!(
            !record.signature_valid(Network::Testnet),
            "bound to one network"
        );
        let mut padded = memo;
        padded[511] = 1;
        assert_eq!(NameRecord::decode(&padded), None);
        let mut upper = memo;
        upper[10] = b'M';
        assert_eq!(NameRecord::decode(&upper), None);
        assert_eq!(NameRecord::decode(&[0; 512]), None);
        let mut text = [0u8; 512];
        text[..5].copy_from_slice(b"hello");
        assert_eq!(NameRecord::decode(&text), None);
    }

    #[test]
    fn burn_address_is_deterministic_per_network_and_recognised() {
        let devnet = burn_address(Network::Devnet);
        assert_eq!(devnet, burn_address(Network::Devnet));
        assert_ne!(
            devnet.to_raw_address_bytes(),
            burn_address(Network::Testnet).to_raw_address_bytes()
        );
        assert!(is_burn_address(Network::Devnet, &devnet));
        assert!(is_burn_address(
            Network::Devnet,
            &burn_viewing_key(Network::Devnet).address_at(5u32, Scope::External)
        ));
        let other = Address::from_raw_address_bytes(&receiver()).unwrap();
        assert!(!is_burn_address(Network::Devnet, &other));
    }

    #[test]
    fn first_registration_wins_and_only_the_owner_updates() {
        let alice = SigningKey::from_bytes(&[1; 32]);
        let bob = SigningKey::from_bytes(&[2; 32]);
        let mut names = BTreeMap::new();
        let price = registration_price("maximus");
        assert_eq!(
            apply(
                &mut names,
                Network::Devnet,
                &operation(signed(OP_REGISTER, "maximus", 0, &alice), price - 1),
                5
            ),
            Err("burned amount below the name price")
        );
        assert!(names.is_empty());
        apply(
            &mut names,
            Network::Devnet,
            &operation(signed(OP_REGISTER, "maximus", 0, &alice), price),
            6,
        )
        .unwrap();
        assert_eq!(
            apply(
                &mut names,
                Network::Devnet,
                &operation(signed(OP_REGISTER, "maximus", 0, &bob), price * 5),
                7
            ),
            Err("name already registered")
        );
        assert_eq!(names["maximus"].owner, alice.verifying_key().to_bytes());
        assert_eq!(
            apply(
                &mut names,
                Network::Devnet,
                &operation(signed(OP_UPDATE, "maximus", 1, &bob), GNR),
                8
            ),
            Err("name belongs to another owner")
        );
        assert_eq!(
            apply(
                &mut names,
                Network::Devnet,
                &operation(signed(OP_UPDATE, "maximus", 2, &alice), GNR),
                8
            ),
            Err("update sequence must follow the current one")
        );
        apply(
            &mut names,
            Network::Devnet,
            &operation(signed(OP_UPDATE, "maximus", 1, &alice), GNR),
            9,
        )
        .unwrap();
        assert_eq!(names["maximus"].sequence, 1);
        assert_eq!(names["maximus"].registered_height, 6);
        assert_eq!(names["maximus"].updated_height, 9);
        let mut forged = signed(OP_REGISTER, "another", 0, &alice);
        forged.receiver[0] ^= 1;
        assert_eq!(
            apply(
                &mut names,
                Network::Devnet,
                &operation(forged, 100 * GNR),
                10
            ),
            Err("invalid owner signature")
        );
    }
}
