//! Shielded addresses and note encryption.
//!
//! An address carries the recipient's owner tag (a Rescue hash of its spend
//! authorization and nullifier keys) and its hybrid X25519 + ML-KEM-1024 public
//! key. Each output encapsulates a fresh key to that public key and encrypts the
//! note (value, randomness, memo) with XChaCha20-Poly1305.

use crate::params::Network;
use crate::types::{MEMO_LEN, NOTE_CIPHERTEXT_LEN, NOTE_PLAINTEXT_LEN};
use crate::{CoreError, Hash32, Result};
use krypton_crypto::field::{Digest, digest_from_bytes, digest_to_bytes};
use krypton_crypto::hash::tagged;
use krypton_crypto::{aead, kem, rescue};

/// Version 2: a 128-bit SHA3 checksum detects any typing or copying error and makes
/// a deliberately altered address that still verifies infeasible to construct.
pub const ADDRESS_VERSION: u8 = 2;
const CHECKSUM_LEN: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub network: Network,
    pub owner: Digest,
    pub kem: kem::PublicKey,
}

fn b32_encode(b: &[u8]) -> String {
    data_encoding_lower(b)
}

// RFC 4648 base32, lowercase, no padding.
const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

fn data_encoding_lower(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 8 / 5 + 1);
    let mut buf: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn b32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buf: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = ALPHABET.iter().position(|&a| a == c.to_ascii_lowercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    (buf == 0).then_some(out)
}

impl Address {
    pub fn payload(&self) -> Vec<u8> {
        let mut v = vec![ADDRESS_VERSION, self.network.id()];
        v.extend_from_slice(&digest_to_bytes(&self.owner));
        v.extend_from_slice(&self.kem.to_bytes());
        v
    }
    pub fn encode(&self) -> String {
        let mut p = self.payload();
        let c = tagged("Krypton/address-checksum/v2", &p);
        p.extend_from_slice(&c[..CHECKSUM_LEN]);
        format!(
            "{}1{}",
            self.network.params().address_prefix,
            b32_encode(&p)
        )
    }
    pub fn decode(s: &str) -> Result<Self> {
        let s = s.trim();
        let (prefix, data) = s
            .split_once('1')
            .ok_or(CoreError::Decode("address separator"))?;
        let bytes = b32_decode(data).ok_or(CoreError::Decode("address base32"))?;
        if bytes.len() < 2 + 32 + CHECKSUM_LEN {
            return Err(CoreError::Decode("address too short"));
        }
        if bytes.first() == Some(&1) {
            return Err(CoreError::Decode(
                "old address format (version 1); ask the recipient for an address from the current wallet",
            ));
        }
        let (p, c) = bytes.split_at(bytes.len() - CHECKSUM_LEN);
        if tagged("Krypton/address-checksum/v2", p)[..CHECKSUM_LEN] != *c {
            return Err(CoreError::Decode("address checksum"));
        }
        if p[0] != ADDRESS_VERSION {
            return Err(CoreError::Decode("unsupported address version"));
        }
        if p.len() != 2 + 32 + kem::PUBLIC_KEY_LEN {
            return Err(CoreError::Decode("address length"));
        }
        let network = Network::from_id(p[1]).ok_or(CoreError::Decode("address network"))?;
        if network.params().address_prefix != prefix {
            return Err(CoreError::Decode(
                "address prefix does not match its network",
            ));
        }
        let owner = digest_from_bytes(p[2..34].try_into().expect("32"))
            .map_err(|_| CoreError::Decode("address owner"))?;
        let kem = kem::PublicKey::from_bytes(&p[34..])
            .map_err(|_| CoreError::Decode("address KEM key"))?;
        Ok(Self {
            network,
            owner,
            kem,
        })
    }
    /// 32-byte content hash; a short address `<prefix>s1...` resolves to the full
    /// address through any node, and the hash makes substitution impossible.
    pub fn short_id(&self) -> Hash32 {
        tagged("Krypton/short-address", &self.payload())
    }
    pub fn short(&self) -> String {
        format!(
            "{}s1{}",
            self.network.params().address_prefix,
            b32_encode(&self.short_id())
        )
    }
    pub fn parse_short(s: &str) -> Result<(Network, Hash32)> {
        let (prefix, data) = s
            .trim()
            .split_once("s1")
            .ok_or(CoreError::Decode("short address"))?;
        let net = [Network::Mainnet, Network::Testnet, Network::Devnet]
            .into_iter()
            .find(|n| n.params().address_prefix == prefix)
            .ok_or(CoreError::Decode("short address prefix"))?;
        let b = b32_decode(data).ok_or(CoreError::Decode("short address base32"))?;
        Ok((
            net,
            b.try_into()
                .map_err(|_| CoreError::Decode("short address length"))?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotePlain {
    pub value: u64,
    pub rcm: Digest,
    pub memo: [u8; MEMO_LEN],
}

impl NotePlain {
    pub fn to_bytes(&self) -> [u8; NOTE_PLAINTEXT_LEN] {
        let mut b = [0u8; NOTE_PLAINTEXT_LEN];
        b[..8].copy_from_slice(&self.value.to_le_bytes());
        b[8..40].copy_from_slice(&digest_to_bytes(&self.rcm));
        b[40..].copy_from_slice(&self.memo);
        b
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != NOTE_PLAINTEXT_LEN {
            return None;
        }
        Some(Self {
            value: u64::from_le_bytes(b[..8].try_into().ok()?),
            rcm: digest_from_bytes(b[8..40].try_into().ok()?).ok()?,
            memo: b[40..].try_into().ok()?,
        })
    }
    pub fn memo_text(&self) -> String {
        let end = self.memo.iter().position(|&c| c == 0).unwrap_or(MEMO_LEN);
        String::from_utf8_lossy(&self.memo[..end]).into_owned()
    }
}

pub fn memo_from_str(s: &str) -> [u8; MEMO_LEN] {
    let mut m = [0u8; MEMO_LEN];
    let b = s.as_bytes();
    let n = b.len().min(MEMO_LEN);
    m[..n].copy_from_slice(&b[..n]);
    m
}

fn nonce_for(cm: &Hash32) -> [u8; 24] {
    tagged("Krypton/note-nonce", cm)[..24]
        .try_into()
        .expect("24")
}

/// Encrypts `note` to `to`; returns (commitment, ciphertext).
pub fn encrypt_note(to: &Address, note: &NotePlain) -> Result<(Hash32, Vec<u8>)> {
    encrypt_note_disclosable(to, note).map(|(cm, ct, _)| (cm, ct))
}

/// Like [`encrypt_note`], also returning the note's one-time key. Revealing that key
/// lets anyone decrypt exactly this output: the basis of a payment proof.
pub fn encrypt_note_disclosable(
    to: &Address,
    note: &NotePlain,
) -> Result<(Hash32, Vec<u8>, zeroize::Zeroizing<[u8; 32]>)> {
    encrypt_note_for(&to.kem, &to.owner, note)
}

/// Encrypts `note` to `kem` with its commitment under `owner`. Lock notes are committed
/// under the lock's owner tag and encrypted to the refunder, who recovers them from the
/// recovery phrase with [`decrypt_note_unchecked`].
pub fn encrypt_note_for(
    kem: &kem::PublicKey,
    owner: &Digest,
    note: &NotePlain,
) -> Result<(Hash32, Vec<u8>, zeroize::Zeroizing<[u8; 32]>)> {
    let cm = digest_to_bytes(&rescue::note_commitment(owner, note.value, &note.rcm));
    let (ct, ss) = kem::encapsulate(kem).map_err(|_| CoreError::Invalid("encapsulation failed"))?;
    let mut out = ct;
    out.extend_from_slice(&aead::seal(&ss, &nonce_for(&cm), &cm, &note.to_bytes()));
    debug_assert_eq!(out.len(), NOTE_CIPHERTEXT_LEN);
    Ok((cm, out, ss))
}

/// Decrypts an output without checking whose note it is. The caller must check that
/// the note re-commits to `cm` under the owner it expects.
pub fn decrypt_note_unchecked(sk: &kem::SecretKey, cm: &Hash32, ct: &[u8]) -> Option<NotePlain> {
    if ct.len() != NOTE_CIPHERTEXT_LEN {
        return None;
    }
    let (kct, body) = ct.split_at(kem::CIPHERTEXT_LEN);
    let ss = sk.decapsulate(kct).ok()?;
    NotePlain::from_bytes(&aead::open(&ss, &nonce_for(cm), cm, body).ok()?)
}

/// Opens an output with its disclosed one-time key; the note must re-commit to `cm`
/// under `owner`.
pub fn open_disclosed_note(
    key: &[u8; 32],
    owner: &Digest,
    cm: &Hash32,
    ct: &[u8],
) -> Option<NotePlain> {
    if ct.len() != NOTE_CIPHERTEXT_LEN {
        return None;
    }
    let pt = aead::open(key, &nonce_for(cm), cm, &ct[kem::CIPHERTEXT_LEN..]).ok()?;
    let note = NotePlain::from_bytes(&pt)?;
    (digest_to_bytes(&rescue::note_commitment(owner, note.value, &note.rcm)) == *cm).then_some(note)
}

pub fn base32_encode(b: &[u8]) -> String {
    b32_encode(b)
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    b32_decode(s)
}

/// Trial decryption by the recipient. Succeeds only if the note decrypts *and*
/// re-commits to `cm` under our owner tag, so a sender cannot forge note contents.
pub fn try_decrypt_note(
    sk: &kem::SecretKey,
    owner: &Digest,
    cm: &Hash32,
    ct: &[u8],
) -> Option<NotePlain> {
    let note = decrypt_note_unchecked(sk, cm, ct)?;
    (digest_to_bytes(&rescue::note_commitment(owner, note.value, &note.rcm)) == *cm).then_some(note)
}

/// Random bytes of ciphertext length for dummy outputs (indistinguishable padding).
pub fn dummy_ciphertext() -> Vec<u8> {
    use rand_core::RngCore;
    let mut v = vec![0u8; NOTE_CIPHERTEXT_LEN];
    rand_core::OsRng.fill_bytes(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use krypton_crypto::rescue::SpendKeys;
    #[test]
    fn address_roundtrip_and_note_encryption() {
        let keys = SpendKeys::from_sk(rescue::scalar(5));
        let ksk = kem::SecretKey::from_seed(&[1; 96]);
        let a = Address {
            network: Network::Testnet,
            owner: keys.owner,
            kem: ksk.public().clone(),
        };
        let s = a.encode();
        assert!(s.starts_with("tgnr1"));
        assert_eq!(Address::decode(&s).unwrap(), a);
        let mut bad = s.clone().into_bytes();
        let i = bad.len() - 3;
        bad[i] = if bad[i] == b'a' { b'b' } else { b'a' };
        assert!(Address::decode(std::str::from_utf8(&bad).unwrap()).is_err());
        let (net, id) = Address::parse_short(&a.short()).unwrap();
        assert_eq!((net, id), (Network::Testnet, a.short_id()));

        let note = NotePlain {
            value: 42,
            rcm: rescue::scalar(9),
            memo: memo_from_str("hello"),
        };
        let (cm, ct) = encrypt_note(&a, &note).unwrap();
        let got = try_decrypt_note(&ksk, &keys.owner, &cm, &ct).unwrap();
        assert_eq!(got, note);
        assert_eq!(got.memo_text(), "hello");
        let other = kem::SecretKey::from_seed(&[2; 96]);
        assert!(try_decrypt_note(&other, &keys.owner, &cm, &ct).is_none());
        assert!(try_decrypt_note(&ksk, &keys.owner, &[0; 32], &ct).is_none());
    }
}
