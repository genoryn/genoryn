//! Password hashing (Argon2id) and deterministic key derivation.

use crate::{CryptoError, Result};
use zeroize::Zeroizing;

/// Argon2id parameters stored next to every encrypted wallet file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Argon2Params {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Argon2Params {
    /// 256 MiB, 3 passes: strong for desktop wallets.
    pub const STRONG: Self = Self {
        m_cost_kib: 256 * 1024,
        t_cost: 3,
        p_cost: 4,
    };
    /// Used only by tests.
    pub const TEST: Self = Self {
        m_cost_kib: 1024,
        t_cost: 1,
        p_cost: 1,
    };
}

pub fn argon2id(password: &[u8], salt: &[u8; 32], p: Argon2Params) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(p.m_cost_kib, p.t_cost, p.p_cost, Some(32))
        .map_err(|_| CryptoError::Failed("argon2 parameters"))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    a.hash_password_into(password, salt, out.as_mut())
        .map_err(|_| CryptoError::Failed("argon2"))?;
    Ok(out)
}

/// Derives `N` bytes for `purpose` from a master seed with SHAKE256.
pub fn derive<const N: usize>(seed: &[u8], purpose: &str) -> Zeroizing<[u8; N]> {
    let mut out = Zeroizing::new([0u8; N]);
    crate::hash::shake(&format!("Krypton/KDF/{purpose}"), seed, out.as_mut());
    out
}
