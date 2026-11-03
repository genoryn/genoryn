//! Hybrid key encapsulation: X25519 + ML-KEM-1024 (FIPS 203, NIST level 5).
//!
//! The shared secret is a SHA3 combination of both component secrets and the full
//! transcript, so it stays secret as long as *either* X25519 (classical) or
//! ML-KEM-1024 (post-quantum) is unbroken.

use crate::{CryptoError, Result, hash};
use fips203::ml_kem_1024;
use fips203::traits::{Decaps, Encaps, KeyGen, SerDes};
use rand_core::{CryptoRngCore, OsRng};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const MLKEM_EK_LEN: usize = ml_kem_1024::EK_LEN;
pub const MLKEM_CT_LEN: usize = ml_kem_1024::CT_LEN;
pub const PUBLIC_KEY_LEN: usize = 32 + MLKEM_EK_LEN;
pub const CIPHERTEXT_LEN: usize = 32 + MLKEM_CT_LEN;

#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey {
    pub x25519: [u8; 32],
    pub mlkem: Box<[u8; MLKEM_EK_LEN]>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretKey {
    x25519: [u8; 32],
    mlkem: Box<[u8; ml_kem_1024::DK_LEN]>,
    #[zeroize(skip)]
    public: PublicKey,
}

impl core::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "HybridKemPublicKey({})",
            hex::encode(&hash::tagged("kem-fp", &self.to_bytes())[..8])
        )
    }
}

impl PublicKey {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(PUBLIC_KEY_LEN);
        v.extend_from_slice(&self.x25519);
        v.extend_from_slice(self.mlkem.as_ref());
        v
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() != PUBLIC_KEY_LEN {
            return Err(CryptoError::Encoding("hybrid KEM public key length"));
        }
        let mut mlkem = Box::new([0u8; MLKEM_EK_LEN]);
        mlkem.copy_from_slice(&b[32..]);
        ml_kem_1024::EncapsKey::try_from_bytes(*mlkem)
            .map_err(|_| CryptoError::Encoding("ML-KEM encapsulation key"))?;
        Ok(Self {
            x25519: b[..32].try_into().expect("32"),
            mlkem,
        })
    }
}

impl SecretKey {
    /// Deterministic key generation from 96 bytes of seed material.
    pub fn from_seed(seed: &[u8; 96]) -> Self {
        let x = x25519_dalek::StaticSecret::from(<[u8; 32]>::try_from(&seed[..32]).expect("32"));
        let d: [u8; 32] = seed[32..64].try_into().expect("32");
        let z: [u8; 32] = seed[64..96].try_into().expect("32");
        let (ek, dk) = ml_kem_1024::KG::keygen_from_seed(d, z);
        let public = PublicKey {
            x25519: x25519_dalek::PublicKey::from(&x).to_bytes(),
            mlkem: Box::new(ek.into_bytes()),
        };
        Self {
            x25519: x.to_bytes(),
            mlkem: Box::new(dk.into_bytes()),
            public,
        }
    }
    pub fn generate() -> Self {
        let mut seed = Zeroizing::new([0u8; 96]);
        OsRng.fill_bytes(seed.as_mut());
        Self::from_seed(&seed)
    }
    pub fn public(&self) -> &PublicKey {
        &self.public
    }

    pub fn decapsulate(&self, ct: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        if ct.len() != CIPHERTEXT_LEN {
            return Err(CryptoError::Encoding("hybrid KEM ciphertext length"));
        }
        let eph: [u8; 32] = ct[..32].try_into().expect("32");
        let x = x25519_dalek::StaticSecret::from(self.x25519);
        let ss_x = Zeroizing::new(
            x.diffie_hellman(&x25519_dalek::PublicKey::from(eph))
                .to_bytes(),
        );
        let ct_m = ml_kem_1024::CipherText::try_from_bytes(ct[32..].try_into().expect("len"))
            .map_err(|_| CryptoError::Encoding("ML-KEM ciphertext"))?;
        let dk = ml_kem_1024::DecapsKey::try_from_bytes(*self.mlkem)
            .map_err(|_| CryptoError::Failed("ML-KEM decapsulation key"))?;
        let ss_m = Zeroizing::new(
            dk.try_decaps(&ct_m)
                .map_err(|_| CryptoError::Failed("decaps"))?
                .into_bytes(),
        );
        Ok(combine(&ss_x, &ss_m, ct, &self.public))
    }
}

use rand_core::RngCore;

/// Encapsulates to `pk`, returning (ciphertext, shared secret).
pub fn encapsulate(pk: &PublicKey) -> Result<(Vec<u8>, Zeroizing<[u8; 32]>)> {
    encapsulate_with_rng(pk, &mut OsRng)
}

pub fn encapsulate_with_rng(
    pk: &PublicKey,
    rng: &mut impl CryptoRngCore,
) -> Result<(Vec<u8>, Zeroizing<[u8; 32]>)> {
    let mut e = Zeroizing::new([0u8; 32]);
    rng.fill_bytes(e.as_mut());
    let eph = x25519_dalek::StaticSecret::from(*e);
    let eph_pub = x25519_dalek::PublicKey::from(&eph).to_bytes();
    let ss_x = Zeroizing::new(
        eph.diffie_hellman(&x25519_dalek::PublicKey::from(pk.x25519))
            .to_bytes(),
    );
    let ek = ml_kem_1024::EncapsKey::try_from_bytes(*pk.mlkem)
        .map_err(|_| CryptoError::Encoding("ML-KEM encapsulation key"))?;
    let (ss_m, ct_m) = ek
        .try_encaps_with_rng(rng)
        .map_err(|_| CryptoError::Failed("encaps"))?;
    let ss_m = Zeroizing::new(ss_m.into_bytes());
    let mut ct = Vec::with_capacity(CIPHERTEXT_LEN);
    ct.extend_from_slice(&eph_pub);
    ct.extend_from_slice(&ct_m.into_bytes());
    let ss = combine(&ss_x, &ss_m, &ct, pk);
    Ok((ct, ss))
}

fn combine(ss_x: &[u8; 32], ss_m: &[u8; 32], ct: &[u8], pk: &PublicKey) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(hash::tagged_parts(
        "Krypton/HybridKEM/X25519-MLKEM1024/v1",
        &[ss_m, ss_x, ct, &pk.x25519, pk.mlkem.as_ref()],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hybrid_kem_roundtrip() {
        let sk = SecretKey::from_seed(&[3u8; 96]);
        let (ct, ss) = encapsulate(sk.public()).unwrap();
        assert_eq!(*sk.decapsulate(&ct).unwrap(), *ss);
        let other = SecretKey::from_seed(&[4u8; 96]);
        // ML-KEM uses implicit rejection: a wrong key yields an unrelated secret.
        assert_ne!(*other.decapsulate(&ct).unwrap(), *ss);
        let pk2 = PublicKey::from_bytes(&sk.public().to_bytes()).unwrap();
        assert_eq!(&pk2, sk.public());
    }
}
