//! XChaCha20-Poly1305 authenticated encryption (256-bit keys: quantum-safe margin).

use crate::{CryptoError, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;

pub fn seal(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    XChaCha20Poly1305::new(key.into())
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("encryption of bounded input cannot fail")
}

pub fn open(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_tamper() {
        let k = [7u8; 32];
        let n = [9u8; 24];
        let mut c = seal(&k, &n, b"aad", b"hello");
        assert_eq!(open(&k, &n, b"aad", &c).unwrap(), b"hello");
        assert!(open(&k, &n, b"other", &c).is_err());
        c[0] ^= 1;
        assert!(open(&k, &n, b"aad", &c).is_err());
    }
}
