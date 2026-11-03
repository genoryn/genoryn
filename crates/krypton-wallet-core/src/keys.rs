//! Deterministic key derivation from a recovery phrase.
//!
//! **Krypton phrase v2** (default): 24 tokens `word-NN`, each an English BIP-39 word
//! (11 bits) and a two-digit number 00–63 (6 bits): 408 bits carrying 384 bits of
//! entropy and a 24-bit SHA3 checksum. 384 bits keep a 192-bit margin even against
//! Grover's quantum search. The checksum catches any wrong word or number.
//!
//! seed = SHAKE256(entropy, passphrase) --SHAKE256--> spend secret (Rescue digest),
//! hybrid KEM seed (X25519 + ML-KEM-1024), ML-DSA-87 signing seed.
//!
//! Legacy 24-word BIP-39 phrases (256-bit entropy) remain restorable.

use anyhow::{Result, anyhow, ensure};
use krypton_core::Network;
use krypton_core::address::Address;
use krypton_crypto::field::digest_from_uniform;
use krypton_crypto::hash::tagged;
use krypton_crypto::kdf::derive;
use krypton_crypto::rescue::SpendKeys;
use krypton_crypto::{kem, sign};
use zeroize::Zeroizing;

pub const PHRASE_WORDS: usize = 24;
const ENTROPY_LEN: usize = 48;
const CHECKSUM_BITS: usize = 24;

pub struct Keys {
    pub network: Network,
    pub spend: SpendKeys,
    pub kem: kem::SecretKey,
    pub signing: sign::MlDsaKey,
    pub account: u32,
}

fn words() -> &'static [&'static str; 2048] {
    bip39::Language::English.word_list()
}

fn checksum(entropy: &[u8]) -> u32 {
    let h = tagged("Krypton/phrase/v2/checksum", entropy);
    u32::from_be_bytes([0, h[0], h[1], h[2]])
}

/// 408-bit string -> 24 tokens of 17 bits.
fn encode_v2(entropy: &[u8; ENTROPY_LEN]) -> String {
    let mut bits: Vec<u8> = Vec::with_capacity(408);
    let mut push = |v: u32, n: usize| {
        for i in (0..n).rev() {
            bits.push(((v >> i) & 1) as u8);
        }
    };
    for b in entropy {
        push(*b as u32, 8);
    }
    push(checksum(entropy), CHECKSUM_BITS);
    bits.chunks(17)
        .map(|c| {
            let v = c.iter().fold(0u32, |a, b| (a << 1) | *b as u32);
            format!("{}-{:02}", words()[(v >> 6) as usize], v & 63)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_v2(phrase: &str) -> Result<Zeroizing<[u8; ENTROPY_LEN]>> {
    let tokens: Vec<&str> = phrase.split_whitespace().collect();
    ensure!(
        tokens.len() == PHRASE_WORDS,
        "recovery phrase must have {PHRASE_WORDS} words, found {}",
        tokens.len()
    );
    let mut bits: Vec<u8> = Vec::with_capacity(408);
    for (i, t) in tokens.iter().enumerate() {
        let t = t.to_ascii_lowercase();
        let (w, n) = t
            .rsplit_once('-')
            .or_else(|| t.find(|c: char| c.is_ascii_digit()).map(|p| t.split_at(p)))
            .ok_or_else(|| anyhow!("word {} ({t}) needs its two-digit number", i + 1))?;
        let idx = bip39::Language::English
            .find_word(w)
            .ok_or_else(|| anyhow!("word {} ({w}) is not in the word list", i + 1))?;
        let num: u32 = n
            .parse()
            .ok()
            .filter(|v| *v < 64 && n.len() == 2)
            .ok_or_else(|| anyhow!("word {}: the number must be 00 to 63", i + 1))?;
        let v = ((idx as u32) << 6) | num;
        for k in (0..17).rev() {
            bits.push(((v >> k) & 1) as u8);
        }
    }
    let mut entropy = Zeroizing::new([0u8; ENTROPY_LEN]);
    for (i, byte) in entropy.iter_mut().enumerate() {
        *byte = bits[i * 8..i * 8 + 8].iter().fold(0u8, |a, b| (a << 1) | b);
    }
    let sum = bits[ENTROPY_LEN * 8..]
        .iter()
        .fold(0u32, |a, b| (a << 1) | *b as u32);
    ensure!(
        sum == checksum(entropy.as_ref()),
        "recovery phrase checksum failed: a word or number is wrong"
    );
    Ok(entropy)
}

fn is_v2(phrase: &str) -> bool {
    phrase.chars().any(|c| c.is_ascii_digit())
}

fn clean(phrase: &str) -> Zeroizing<String> {
    Zeroizing::new(
        phrase
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// A fresh Krypton phrase v2 (384-bit entropy).
pub fn new_mnemonic() -> Zeroizing<String> {
    let mut entropy = Zeroizing::new([0u8; ENTROPY_LEN]);
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, entropy.as_mut());
    Zeroizing::new(encode_v2(&entropy))
}

pub fn validate_mnemonic(m: &str) -> Result<()> {
    if is_v2(m) {
        decode_v2(m).map(|_| ())
    } else {
        let p = bip39::Mnemonic::parse_normalized(&clean(m))
            .map_err(|e| anyhow!("invalid recovery phrase: {e}"))?;
        ensure!(p.word_count() == 24, "recovery phrase must have 24 words");
        Ok(())
    }
}

/// Canonical form of a phrase (lowercase, single spaces, `word-NN`).
pub fn normalize(m: &str) -> Result<Zeroizing<String>> {
    if is_v2(m) {
        Ok(Zeroizing::new(encode_v2(&*decode_v2(m)?)))
    } else {
        validate_mnemonic(m)?;
        Ok(clean(m))
    }
}

fn seed_of(m: &str, passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    if is_v2(m) {
        let e = decode_v2(m)?;
        let mut ikm = Zeroizing::new(e.to_vec());
        ikm.push(0);
        ikm.extend_from_slice(passphrase.as_bytes());
        Ok(Zeroizing::new(
            derive::<64>(&ikm, "Krypton/phrase/v2/seed").to_vec(),
        ))
    } else {
        validate_mnemonic(m)?;
        let mn = bip39::Mnemonic::parse_normalized(&clean(m)).map_err(|e| anyhow!("{e}"))?;
        Ok(Zeroizing::new(mn.to_seed(passphrase).to_vec()))
    }
}

impl Keys {
    pub fn from_mnemonic(
        m: &str,
        passphrase: &str,
        network: Network,
        account: u32,
    ) -> Result<Self> {
        let seed = seed_of(m, passphrase)?;
        let ctx = |p: &str| format!("{}/{}/{}", network.as_str(), account, p);
        let sk = derive::<32>(seed.as_ref(), &ctx("spend"));
        let kem_seed = derive::<96>(seed.as_ref(), &ctx("kem"));
        let sig_seed = derive::<32>(seed.as_ref(), &ctx("mldsa"));
        Ok(Self {
            network,
            spend: SpendKeys::from_sk(digest_from_uniform(&sk)),
            kem: kem::SecretKey::from_seed(&kem_seed),
            signing: sign::MlDsaKey::from_seed(&sig_seed),
            account,
        })
    }
    pub fn address(&self) -> Address {
        Address {
            network: self.network,
            owner: self.spend.owner,
            kem: self.kem.public().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_and_network_separated() {
        let m = new_mnemonic();
        let a = Keys::from_mnemonic(&m, "", Network::Testnet, 0).unwrap();
        let b = Keys::from_mnemonic(&m, "", Network::Testnet, 0).unwrap();
        assert_eq!(a.address(), b.address());
        let c = Keys::from_mnemonic(&m, "", Network::Mainnet, 0).unwrap();
        assert_ne!(a.spend.owner, c.spend.owner);
        let d = Keys::from_mnemonic(&m, "extra", Network::Testnet, 0).unwrap();
        assert_ne!(a.spend.owner, d.spend.owner);
        assert!(Keys::from_mnemonic("abandon abandon", "", Network::Testnet, 0).is_err());
    }

    #[test]
    fn phrase_v2_format_roundtrip_and_checksum() {
        for _ in 0..50 {
            let m = new_mnemonic();
            let toks: Vec<&str> = m.split(' ').collect();
            assert_eq!(toks.len(), 24);
            assert!(toks.iter().all(|t| {
                let (w, n) = t.rsplit_once('-').unwrap();
                words().contains(&w) && n.len() == 2 && n.parse::<u32>().unwrap() < 64
            }));
            let e = decode_v2(&m).unwrap();
            assert_eq!(encode_v2(&e), *m);
            assert_eq!(e.len() * 8, 384);
            // Accepted without hyphens and in capitals; normalized back.
            let loose = m.replace('-', "").to_uppercase();
            assert_eq!(*normalize(&loose).unwrap(), *m);
            assert_eq!(
                Keys::from_mnemonic(&loose, "", Network::Mainnet, 0)
                    .unwrap()
                    .address(),
                Keys::from_mnemonic(&m, "", Network::Mainnet, 0)
                    .unwrap()
                    .address()
            );
        }
    }

    #[test]
    fn phrase_v2_rejects_any_single_change() {
        let m = new_mnemonic();
        let toks: Vec<String> = m.split(' ').map(String::from).collect();
        let mut rejected = 0;
        for i in 0..24 {
            let (w, n) = toks[i].rsplit_once('-').unwrap();
            let n2 = (n.parse::<u32>().unwrap() + 1) % 64;
            let mut t = toks.clone();
            t[i] = format!("{w}-{n2:02}");
            rejected += validate_mnemonic(&t.join(" ")).is_err() as u32;
            let mut t = toks.clone();
            let other = if w == "abandon" { "ability" } else { "abandon" };
            t[i] = format!("{other}-{n}");
            rejected += validate_mnemonic(&t.join(" ")).is_err() as u32;
        }
        assert_eq!(
            rejected, 48,
            "a 24-bit checksum misses one change in 16 million"
        );
        let mut short = toks.clone();
        short.pop();
        assert!(validate_mnemonic(&short.join(" ")).is_err());
        let mut big = toks.clone();
        big[0] = format!("{}-64", big[0].rsplit_once('-').unwrap().0);
        assert!(validate_mnemonic(&big.join(" ")).is_err());
    }

    #[test]
    fn legacy_bip39_phrases_still_restore() {
        let legacy = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        validate_mnemonic(legacy).unwrap();
        let a = Keys::from_mnemonic(legacy, "", Network::Mainnet, 0).unwrap();
        let b = Keys::from_mnemonic(&legacy.to_uppercase(), "", Network::Mainnet, 0).unwrap();
        assert_eq!(a.address(), b.address());
    }
}
