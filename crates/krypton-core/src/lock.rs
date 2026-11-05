//! Lock notes: the GNR side of BTC ⇄ GNR atomic swaps (`docs/spec/atomic-swaps.md`).
//!
//! A lock note is an ordinary note whose key is derived from public swap terms, with its
//! owner tag hashed under the `LOCK_OWNER` domain so no ordinary spend can take it. It
//! can only be spent by a lock spend, and consensus pins where that spend pays:
//!
//! * **claim**: reveals the 32-byte secret `s` with `SHA-256(s) = hash`, must expire
//!   before `refund_height`, and pays exactly `claim_outputs`;
//! * **refund**: valid from `refund_height` on, pays exactly `refund_outputs`.
//!
//! Anyone who learns the terms could build the key, but the outputs are fixed by the
//! terms and only their creator knows their openings, so nobody else can redirect the
//! coins.

use crate::codec::{Reader, Writer};
use crate::{CoreError, Hash32, Result};
use krypton_crypto::field::{Digest, digest_from_uniform, digest_to_bytes};
use krypton_crypto::hash::tagged_parts;
use krypton_crypto::rescue::{self, SpendKeys};
use sha2::{Digest as _, Sha256};

/// Terms of one lock note, agreed by both sides of a swap.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockTerms {
    /// SHA-256 of the swap secret (the same hash lock as on Bitcoin).
    #[serde(with = "hex::serde")]
    pub hash: [u8; 32],
    /// First height at which the refund may be mined; claims must expire before it.
    pub refund_height: u64,
    /// Output commitments a claim must create (chosen by the claimer).
    pub claim_outputs: [HexCm; 2],
    /// Output commitments a refund must create (chosen by the refunder).
    pub refund_outputs: [HexCm; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HexCm(#[serde(with = "hex::serde")] pub Hash32);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockMode {
    Claim {
        #[serde(with = "hex::serde")]
        secret: [u8; 32],
    },
    Refund,
}

/// The public part of a lock spend (input 0 of the transaction).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockSpend {
    pub terms: LockTerms,
    pub mode: LockMode,
}

pub fn sha256(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

impl LockTerms {
    fn material(&self, tag: &str) -> [u8; 32] {
        tagged_parts(
            tag,
            &[
                &self.hash,
                &self.refund_height.to_le_bytes(),
                &self.claim_outputs[0].0,
                &self.claim_outputs[1].0,
                &self.refund_outputs[0].0,
                &self.refund_outputs[1].0,
            ],
        )
    }
    /// Spending key of the lock note (known to anyone who knows the terms).
    pub fn keys(&self) -> SpendKeys {
        SpendKeys::from_sk(digest_from_uniform(&self.material("Krypton/lock/sk/v1")))
    }
    /// Owner tag of the lock note (`H(LOCK_OWNER; ak, nk)`).
    pub fn owner(&self) -> Digest {
        let k = self.keys();
        rescue::lock_owner_of(&k.ak, &k.nk)
    }
    /// Commitment randomness of the lock note: both sides derive it from the terms.
    pub fn rcm(&self) -> Digest {
        digest_from_uniform(&self.material("Krypton/lock/rcm/v1"))
    }
    /// Commitment of the lock note holding `value`.
    pub fn commitment(&self, value: u64) -> Hash32 {
        digest_to_bytes(&rescue::note_commitment(&self.owner(), value, &self.rcm()))
    }
    pub fn nullifier(&self, pos: u64) -> Hash32 {
        digest_to_bytes(&rescue::nullifier(&self.keys().nk, pos))
    }

    pub(crate) fn encode(&self, w: &mut Writer) {
        w.raw(&self.hash).u64(self.refund_height);
        for c in self.claim_outputs.iter().chain(self.refund_outputs.iter()) {
            w.raw(&c.0);
        }
    }
    pub(crate) fn decode(r: &mut Reader) -> Result<Self> {
        let hash = r.array()?;
        let refund_height = r.u64()?;
        let mut cm = || -> Result<HexCm> { Ok(HexCm(r.array()?)) };
        Ok(Self {
            hash,
            refund_height,
            claim_outputs: [cm()?, cm()?],
            refund_outputs: [cm()?, cm()?],
        })
    }
}

impl LockSpend {
    pub(crate) fn encode(&self, w: &mut Writer) {
        self.terms.encode(w);
        match &self.mode {
            LockMode::Claim { secret } => {
                w.u8(0).raw(secret);
            }
            LockMode::Refund => {
                w.u8(1);
            }
        }
    }
    pub(crate) fn decode(r: &mut Reader) -> Result<Self> {
        let terms = LockTerms::decode(r)?;
        let mode = match r.u8()? {
            0 => LockMode::Claim { secret: r.array()? },
            1 => LockMode::Refund,
            _ => return Err(CoreError::Decode("unknown lock mode")),
        };
        Ok(Self { terms, mode })
    }

    /// Rules that need no chain state: the secret, the expiry and the pinned outputs.
    pub fn check(&self, outputs: [&Hash32; 2], expiry_height: u64) -> Result<()> {
        let bad = |m| Err(CoreError::Invalid(m));
        let t = &self.terms;
        let want = match &self.mode {
            LockMode::Claim { secret } => {
                if sha256(secret) != t.hash {
                    return bad("the secret does not match the lock");
                }
                if expiry_height == 0 || expiry_height >= t.refund_height {
                    return bad("a claim must expire before the refund height");
                }
                &t.claim_outputs
            }
            LockMode::Refund => &t.refund_outputs,
        };
        if *outputs[0] != want[0].0 || *outputs[1] != want[1].0 {
            return bad("the lock spend pays other outputs than its terms");
        }
        Ok(())
    }

    /// Rules at the height the spend is mined.
    pub fn check_height(&self, height: u64, activation: u64) -> Result<()> {
        if height < activation {
            return Err(CoreError::Invalid("lock spends are not active yet"));
        }
        if self.mode == LockMode::Refund && height < self.terms.refund_height {
            return Err(CoreError::Invalid(
                "the refund is not valid before its height",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms() -> (LockTerms, [u8; 32]) {
        let secret = [7u8; 32];
        let t = LockTerms {
            hash: sha256(&secret),
            refund_height: 1_000,
            claim_outputs: [HexCm([1; 32]), HexCm([2; 32])],
            refund_outputs: [HexCm([3; 32]), HexCm([4; 32])],
        };
        (t, secret)
    }

    #[test]
    fn claim_rules() {
        let (terms, secret) = terms();
        let s = LockSpend {
            terms: terms.clone(),
            mode: LockMode::Claim { secret },
        };
        assert!(s.check([&[1; 32], &[2; 32]], 999).is_ok());
        assert!(
            s.check([&[1; 32], &[2; 32]], 1_000).is_err(),
            "expires too late"
        );
        assert!(s.check([&[1; 32], &[2; 32]], 0).is_err(), "never expires");
        assert!(
            s.check([&[3; 32], &[4; 32]], 999).is_err(),
            "refund outputs"
        );
        assert!(
            s.check([&[2; 32], &[1; 32]], 999).is_err(),
            "swapped outputs"
        );
        let wrong = LockSpend {
            terms,
            mode: LockMode::Claim { secret: [8; 32] },
        };
        assert!(
            wrong.check([&[1; 32], &[2; 32]], 999).is_err(),
            "wrong secret"
        );
        assert!(s.check_height(5, 0).is_ok());
        assert!(s.check_height(5, 6).is_err(), "before activation");
    }

    #[test]
    fn refund_rules() {
        let (terms, _) = terms();
        let s = LockSpend {
            terms,
            mode: LockMode::Refund,
        };
        assert!(s.check([&[3; 32], &[4; 32]], 0).is_ok());
        assert!(s.check([&[1; 32], &[2; 32]], 0).is_err(), "claim outputs");
        assert!(s.check_height(999, 0).is_err(), "too early");
        assert!(s.check_height(1_000, 0).is_ok());
    }

    #[test]
    fn keys_depend_on_every_term_and_differ_from_ordinary_owners() {
        let (t, _) = terms();
        let mut seen = vec![t.owner()];
        let mut v = t.clone();
        v.hash[0] ^= 1;
        seen.push(v.owner());
        let mut v = t.clone();
        v.refund_height += 1;
        seen.push(v.owner());
        let mut v = t.clone();
        v.claim_outputs[1].0[0] ^= 1;
        seen.push(v.owner());
        let mut v = t.clone();
        v.refund_outputs[0].0[0] ^= 1;
        seen.push(v.owner());
        seen.push(t.keys().owner);
        let n = seen.len();
        seen.sort_by_key(digest_to_bytes);
        seen.dedup();
        assert_eq!(seen.len(), n);
        assert_ne!(t.commitment(5), t.commitment(6));
    }

    #[test]
    fn encoding_roundtrip() {
        let (terms, secret) = terms();
        for mode in [LockMode::Claim { secret }, LockMode::Refund] {
            let s = LockSpend {
                terms: terms.clone(),
                mode,
            };
            let mut w = Writer::default();
            s.encode(&mut w);
            let mut r = Reader::new(&w.0);
            assert_eq!(LockSpend::decode(&mut r).unwrap(), s);
            r.finish().unwrap();
        }
    }
}
