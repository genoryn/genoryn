//! Post-quantum signatures.
//!
//! * [`MlDsaKey`]: ML-DSA-87 (FIPS 204, NIST level 5), fast, used for node identity
//!   and payment proofs.
//! * [`HybridKey`]: ML-DSA-87 **and** SLH-DSA-SHAKE-256s (FIPS 205). A hybrid
//!   signature is valid only if both components verify, so it survives a break of
//!   lattices (SLH-DSA is purely hash-based). Used for release signing.

use crate::{CryptoError, Result};
use fips204::ml_dsa_87;
use fips204::traits::{KeyGen as _, SerDes as _, Signer as _, Verifier as _};
use fips205::slh_dsa_shake_256s;
use fips205::traits::{SerDes as SlhSerDes, Signer as SlhSigner, Verifier as SlhVerifier};

pub const MLDSA_PK_LEN: usize = ml_dsa_87::PK_LEN;
pub const MLDSA_SIG_LEN: usize = ml_dsa_87::SIG_LEN;
pub const SLH_PK_LEN: usize = slh_dsa_shake_256s::PK_LEN;
pub const SLH_SIG_LEN: usize = slh_dsa_shake_256s::SIG_LEN;

pub struct MlDsaKey {
    sk: ml_dsa_87::PrivateKey,
    pk: ml_dsa_87::PublicKey,
}

impl MlDsaKey {
    pub fn from_seed(xi: &[u8; 32]) -> Self {
        let (pk, sk) = ml_dsa_87::KG::keygen_from_seed(xi);
        Self { sk, pk }
    }
    pub fn public_bytes(&self) -> Vec<u8> {
        self.pk.clone().into_bytes().to_vec()
    }
    pub fn sign(&self, msg: &[u8], ctx: &[u8]) -> Result<Vec<u8>> {
        Ok(self
            .sk
            .try_sign(msg, ctx)
            .map_err(|_| CryptoError::Failed("ML-DSA sign"))?
            .to_vec())
    }
}

pub fn mldsa_verify(pk: &[u8], msg: &[u8], sig: &[u8], ctx: &[u8]) -> bool {
    let (Ok(pk), Ok(sig)) = (
        <[u8; MLDSA_PK_LEN]>::try_from(pk),
        <[u8; MLDSA_SIG_LEN]>::try_from(sig),
    ) else {
        return false;
    };
    match ml_dsa_87::PublicKey::try_from_bytes(pk) {
        Ok(pk) => pk.verify(msg, &sig, ctx),
        Err(_) => false,
    }
}

pub struct HybridKey {
    mldsa: MlDsaKey,
    slh_sk: slh_dsa_shake_256s::PrivateKey,
    slh_pk: slh_dsa_shake_256s::PublicKey,
}

impl HybridKey {
    pub fn from_seed(seed: &[u8; 128]) -> Self {
        let xi: [u8; 32] = seed[..32].try_into().expect("32");
        const N: usize = slh_dsa_shake_256s::N;
        let a: &[u8; N] = seed[32..32 + N].try_into().expect("n");
        let b: &[u8; N] = seed[64..64 + N].try_into().expect("n");
        let c: &[u8; N] = seed[96..96 + N].try_into().expect("n");
        let (pk, sk) =
            <slh_dsa_shake_256s::KG as fips205::traits::KeyGen>::keygen_with_seeds(a, b, c);
        Self {
            mldsa: MlDsaKey::from_seed(&xi),
            slh_sk: sk,
            slh_pk: pk,
        }
    }
    /// `mldsa_pk || slh_pk`
    pub fn public_bytes(&self) -> Vec<u8> {
        let mut v = self.mldsa.public_bytes();
        v.extend_from_slice(&self.slh_pk.clone().into_bytes());
        v
    }
    /// `mldsa_sig || slh_sig` (deterministic SLH-DSA variant).
    pub fn sign(&self, msg: &[u8], ctx: &[u8]) -> Result<Vec<u8>> {
        let mut v = self.mldsa.sign(msg, ctx)?;
        let s = self
            .slh_sk
            .try_sign(msg, ctx, false)
            .map_err(|_| CryptoError::Failed("SLH-DSA sign"))?;
        v.extend_from_slice(&s);
        Ok(v)
    }
}

pub fn hybrid_verify(pk: &[u8], msg: &[u8], sig: &[u8], ctx: &[u8]) -> bool {
    if pk.len() != MLDSA_PK_LEN + SLH_PK_LEN || sig.len() != MLDSA_SIG_LEN + SLH_SIG_LEN {
        return false;
    }
    if !mldsa_verify(&pk[..MLDSA_PK_LEN], msg, &sig[..MLDSA_SIG_LEN], ctx) {
        return false;
    }
    let Ok(spk) =
        slh_dsa_shake_256s::PublicKey::try_from_bytes(pk[MLDSA_PK_LEN..].try_into().expect("len"))
    else {
        return false;
    };
    let ssig: [u8; SLH_SIG_LEN] = sig[MLDSA_SIG_LEN..].try_into().expect("len");
    spk.verify(msg, &ssig, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mldsa_sign_verify() {
        let k = MlDsaKey::from_seed(&[1u8; 32]);
        let s = k.sign(b"msg", b"ctx").unwrap();
        assert!(mldsa_verify(&k.public_bytes(), b"msg", &s, b"ctx"));
        assert!(!mldsa_verify(&k.public_bytes(), b"msh", &s, b"ctx"));
        assert!(!mldsa_verify(&k.public_bytes(), b"msg", &s, b"other"));
    }
    #[test]
    #[ignore = "SLH-DSA-256s signing takes several seconds in debug builds"]
    fn hybrid_sign_verify() {
        let k = HybridKey::from_seed(&[2u8; 128]);
        let s = k.sign(b"release", b"Krypton/release").unwrap();
        assert!(hybrid_verify(
            &k.public_bytes(),
            b"release",
            &s,
            b"Krypton/release"
        ));
        assert!(!hybrid_verify(
            &k.public_bytes(),
            b"releasf",
            &s,
            b"Krypton/release"
        ));
    }
}
