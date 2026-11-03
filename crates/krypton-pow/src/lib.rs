//! KryptoHash v1: a memory-hard proof of work designed for GPUs that stays
//! mineable on CPUs and is cheap to verify.
//!
//! * A 16 MiB **cache** is derived per epoch from a seed (sequential BLAKE3 +
//!   three RandMemoHash passes).
//! * A **dataset** (2 GiB at launch, +4 MiB per epoch, capped at 12 GiB) is
//!   derived from the cache: every 64-byte item mixes 64 pseudo-random cache items.
//! * A **hash** reads 64 random 128-byte pairs from the dataset into a 128-byte mix.
//!   Throughput is bound by random memory bandwidth, which GPUs have in abundance
//!   (hundreds of GB/s) and CPUs less so.
//! * **Verification** recomputes only the 128 dataset items touched (from the cache):
//!   well under a millisecond, which keeps light clients fast.
//!
//! The specification lives in `docs/spec/kryptohash.md`.

// Only the GPU backends (FFI kernel launches) opt out of this.
#![deny(unsafe_code)]

pub mod blake;
pub mod cpu;
#[cfg(feature = "cuda")]
pub mod cuda;
#[cfg(feature = "opencl")]
pub mod opencl;

use parking_lot::Mutex;
use rayon::prelude::*;
use std::sync::Arc;

pub const EPOCH_LENGTH: u64 = 16_384;
pub const ACCESSES: usize = 64;
pub const DATASET_PARENTS: u32 = 64;
pub const CACHE_ROUNDS: usize = 3;
const FNV_PRIME: u32 = 0x0100_0193;
const MIX_TWEAK: u32 = 0x9E37_79B9;

#[inline(always)]
pub fn fnv(a: u32, b: u32) -> u32 {
    a.wrapping_mul(FNV_PRIME) ^ b
}

/// Sizes that define one KryptoHash instance (mainnet/testnet use [`PowParams::STANDARD`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowParams {
    pub cache_items: u32,
    pub dataset_init_bytes: u64,
    pub dataset_growth_bytes: u64,
    pub dataset_max_bytes: u64,
}

impl PowParams {
    pub const STANDARD: Self = Self {
        cache_items: 1 << 18,
        dataset_init_bytes: 2 << 30,
        dataset_growth_bytes: 4 << 20,
        dataset_max_bytes: 12 << 30,
    };
    /// Tiny instance for automated tests and local development networks only.
    pub const DEVNET: Self = Self {
        cache_items: 1 << 12,
        dataset_init_bytes: 8 << 20,
        dataset_growth_bytes: 1 << 20,
        dataset_max_bytes: 32 << 20,
    };
    pub fn dataset_items(&self, epoch: u64) -> u64 {
        let bytes = self
            .dataset_init_bytes
            .saturating_add(epoch.saturating_mul(self.dataset_growth_bytes))
            .min(self.dataset_max_bytes);
        (bytes / 128) * 2
    }
    pub fn dataset_pairs(&self, epoch: u64) -> u32 {
        (self.dataset_items(epoch) / 2) as u32
    }
}

pub fn epoch_of(height: u64) -> u64 {
    height / EPOCH_LENGTH
}

/// Epoch seed bound to the chain identity, so datasets never carry over between networks.
pub fn epoch_seed(chain_seed: &[u8; 32], epoch: u64) -> [u8; 32] {
    krypton_crypto::hash::tagged_parts(
        "KryptoHash/v1/epoch-seed",
        &[chain_seed, &epoch.to_le_bytes()],
    )
}

/// Per-epoch cache of 64-byte items.
pub struct Cache {
    pub epoch: u64,
    pub seed: [u8; 32],
    pub items: Vec<[u32; 16]>,
}

impl Cache {
    pub fn new(params: &PowParams, chain_seed: &[u8; 32], epoch: u64) -> Self {
        let seed = epoch_seed(chain_seed, epoch);
        let n = params.cache_items as usize;
        let mut items = vec![[0u32; 16]; n];
        let mut blk = [0u32; 16];
        blake::bytes_to_words(&seed, &mut blk[..8]);
        items[0] = blake::b64(&blk, 32);
        for k in 1..n {
            items[k] = blake::b64(&items[k - 1], 64);
        }
        for _ in 0..CACHE_ROUNDS {
            for k in 0..n {
                let v = items[k][0] as usize % n;
                let prev = items[(k + n - 1) % n];
                let mut x = [0u32; 16];
                for w in 0..16 {
                    x[w] = prev[w] ^ items[v][w];
                }
                items[k] = blake::b64(&x, 64);
            }
        }
        Self { epoch, seed, items }
    }
    pub fn words(&self) -> Vec<u32> {
        self.items.iter().flatten().copied().collect()
    }

    /// Dataset item `i` computed from the cache.
    #[inline]
    pub fn item(&self, i: u32) -> [u32; 16] {
        let n = self.items.len() as u32;
        let mut mix = self.items[(i % n) as usize];
        mix[0] ^= i;
        mix = blake::b64(&mix, 64);
        for j in 0..DATASET_PARENTS {
            let p = fnv(i ^ j, mix[(j & 15) as usize]) % n;
            let c = &self.items[p as usize];
            for w in 0..16 {
                mix[w] = fnv(mix[w], c[w]);
            }
        }
        blake::b64(&mix, 64)
    }
}

/// The full dataset used by CPU miners.
pub struct Dataset {
    pub epoch: u64,
    pub pairs: u32,
    pub items: Vec<[u32; 16]>,
}

impl Dataset {
    pub fn generate(params: &PowParams, cache: &Cache, threads: usize) -> Self {
        let count = params.dataset_items(cache.epoch) as usize;
        let mut items = vec![[0u32; 16]; count];
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .build()
            .expect("thread pool");
        pool.install(|| {
            items
                .par_chunks_mut(4096)
                .enumerate()
                .for_each(|(ci, chunk)| {
                    for (k, it) in chunk.iter_mut().enumerate() {
                        *it = cache.item((ci * 4096 + k) as u32);
                    }
                });
        });
        Self {
            epoch: cache.epoch,
            pairs: (count / 2) as u32,
            items,
        }
    }
}

/// Header digest (32 bytes) as eight little-endian words.
pub type HeaderWords = [u32; 8];

pub fn header_words(h: &[u8; 32]) -> HeaderWords {
    let mut w = [0u32; 8];
    blake::bytes_to_words(h, &mut w);
    w
}

#[inline(always)]
fn seed_state(hdr: &HeaderWords, nonce: u64) -> ([u32; 16], [u32; 32]) {
    let mut blk = [0u32; 16];
    blk[..8].copy_from_slice(hdr);
    blk[8] = nonce as u32;
    blk[9] = (nonce >> 32) as u32;
    let s = blake::b64(&blk, 40);
    let mut mix = [0u32; 32];
    for w in 0..16 {
        mix[w] = s[w];
        mix[w + 16] = s[w] ^ MIX_TWEAK;
    }
    (s, mix)
}

#[inline(always)]
fn access_index(a: usize, s0: u32, mix: &[u32; 32], pairs: u32) -> u32 {
    fnv(a as u32 ^ s0, mix[a & 31]) % pairs
}

#[inline(always)]
fn mix_in(a: usize, mix: &mut [u32; 32], lo: &[u32; 16], hi: &[u32; 16]) {
    let rot = (a as u32 & 15) + 1;
    for w in 0..16 {
        mix[w] = fnv(mix[w], lo[w]).rotate_left(rot);
        mix[w + 16] = fnv(mix[w + 16], hi[w]).rotate_left(rot);
    }
}

#[inline(always)]
fn finish(s: &[u32; 16], mix: &[u32; 32]) -> [u8; 32] {
    let mut c = [0u32; 8];
    for k in 0..8 {
        c[k] = fnv(
            fnv(fnv(mix[4 * k], mix[4 * k + 1]), mix[4 * k + 2]),
            mix[4 * k + 3],
        );
    }
    let out = blake::hash96(s, &c);
    let mut b = [0u8; 32];
    for (i, w) in out.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    b
}

/// Full-dataset hash (miners).
pub fn hash_full(ds: &Dataset, hdr: &HeaderWords, nonce: u64) -> [u8; 32] {
    let (s, mut mix) = seed_state(hdr, nonce);
    for a in 0..ACCESSES {
        let p = access_index(a, s[0], &mix, ds.pairs) as usize;
        mix_in(a, &mut mix, &ds.items[2 * p], &ds.items[2 * p + 1]);
    }
    finish(&s, &mix)
}

/// Cache-only hash (verifiers and light clients).
pub fn hash_light(cache: &Cache, pairs: u32, hdr: &HeaderWords, nonce: u64) -> [u8; 32] {
    let (s, mut mix) = seed_state(hdr, nonce);
    for a in 0..ACCESSES {
        let p = access_index(a, s[0], &mix, pairs);
        let lo = cache.item(2 * p);
        let hi = cache.item(2 * p + 1);
        mix_in(a, &mut mix, &lo, &hi);
    }
    finish(&s, &mix)
}

/// A 256-bit unsigned integer as four little-endian u64 limbs.
pub type U256 = [u64; 4];

/// `hash` (little-endian 256-bit integer) meets `difficulty` iff `hash * difficulty < 2^256`.
pub fn meets_difficulty(hash: &[u8; 32], difficulty: u128) -> bool {
    if difficulty == 0 {
        return false;
    }
    let d = [difficulty as u64, (difficulty >> 64) as u64];
    // Schoolbook 256 x 128 -> 384-bit product in 64-bit limbs; every partial sum
    // (a*b + c + carry, all < 2^64) fits in a u128, so nothing can overflow.
    let mut out = [0u64; 6];
    for i in 0..4 {
        let l = u64::from_le_bytes(hash[i * 8..i * 8 + 8].try_into().expect("8")) as u128;
        let mut carry = 0u128;
        for (j, &dj) in d.iter().enumerate() {
            let t = l * dj as u128 + out[i + j] as u128 + carry;
            out[i + j] = t as u64;
            carry = t >> 64;
        }
        let mut k = i + 2;
        while carry != 0 && k < out.len() {
            let t = out[k] as u128 + carry;
            out[k] = t as u64;
            carry = t >> 64;
            k += 1;
        }
    }
    out[4] == 0 && out[5] == 0
}

/// `floor((2^256 - 1) / difficulty)` as a little-endian 256-bit integer (GPU target).
pub fn target_from_difficulty(difficulty: u128) -> U256 {
    assert!(difficulty > 0);
    let mut q = [0u64; 4];
    let mut rem: u128 = 0;
    let d = difficulty;
    // Long division of the all-ones 256-bit number, one bit at a time.
    for bit in (0..256).rev() {
        let top = rem >> 127;
        rem = (rem << 1) | 1;
        if top == 1 || rem >= d {
            rem = rem.wrapping_sub(d);
            q[bit / 64] |= 1 << (bit % 64);
        }
    }
    q
}

pub fn hash_le_cmp_target(hash: &[u8; 32], target: &U256) -> bool {
    for i in (0..4).rev() {
        let h = u64::from_le_bytes(hash[i * 8..i * 8 + 8].try_into().expect("8"));
        if h != target[i] {
            return h < target[i];
        }
    }
    true
}

/// Work represented by a block of the given difficulty (expected hashes).
pub fn work(difficulty: u128) -> u128 {
    difficulty
}

/// Thread-safe cache of the most recent epoch caches for verification.
pub struct Verifier {
    params: PowParams,
    chain_seed: [u8; 32],
    caches: Mutex<Vec<Arc<Cache>>>,
}

impl Verifier {
    pub fn new(params: PowParams, chain_seed: [u8; 32]) -> Self {
        Self {
            params,
            chain_seed,
            caches: Mutex::new(Vec::new()),
        }
    }
    pub fn params(&self) -> &PowParams {
        &self.params
    }
    pub fn chain_seed(&self) -> &[u8; 32] {
        &self.chain_seed
    }
    pub fn cache(&self, epoch: u64) -> Arc<Cache> {
        if let Some(c) = self.caches.lock().iter().find(|c| c.epoch == epoch) {
            return c.clone();
        }
        let c = Arc::new(Cache::new(&self.params, &self.chain_seed, epoch));
        let mut g = self.caches.lock();
        if !g.iter().any(|x| x.epoch == epoch) {
            g.push(c.clone());
            if g.len() > 3 {
                g.remove(0);
            }
        }
        c
    }
    /// Returns the PoW hash for a header at `height`.
    pub fn pow_hash(&self, height: u64, header_hash: &[u8; 32], nonce: u64) -> [u8; 32] {
        let e = epoch_of(height);
        let cache = self.cache(e);
        hash_light(
            &cache,
            self.params.dataset_pairs(e),
            &header_words(header_hash),
            nonce,
        )
    }
    pub fn verify(
        &self,
        height: u64,
        header_hash: &[u8; 32],
        nonce: u64,
        difficulty: u128,
    ) -> bool {
        meets_difficulty(&self.pow_hash(height, header_hash, nonce), difficulty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_equals_full_and_is_deterministic() {
        let p = PowParams::DEVNET;
        let cache = Cache::new(&p, &[1; 32], 0);
        let ds = Dataset::generate(&p, &cache, 4);
        let hdr = header_words(&[9; 32]);
        for nonce in [0u64, 1, 77, u64::MAX] {
            assert_eq!(
                hash_full(&ds, &hdr, nonce),
                hash_light(&cache, ds.pairs, &hdr, nonce)
            );
        }
        assert_ne!(hash_full(&ds, &hdr, 1), hash_full(&ds, &hdr, 2));
        let other = Cache::new(&p, &[2; 32], 0);
        assert_ne!(
            hash_light(&other, ds.pairs, &hdr, 1),
            hash_full(&ds, &hdr, 1)
        );
    }

    #[test]
    fn difficulty_and_target_agree() {
        for d in [
            1u128,
            2,
            3,
            1000,
            123_456_789,
            u64::MAX as u128,
            1u128 << 100,
        ] {
            let t = target_from_difficulty(d);
            let mut b = [0u8; 32];
            for i in 0..4 {
                b[i * 8..i * 8 + 8].copy_from_slice(&t[i].to_le_bytes());
            }
            assert!(meets_difficulty(&b, d), "target meets {d}");
            assert!(hash_le_cmp_target(&b, &t));
            // target + 1 must fail
            let mut t1 = t;
            for l in t1.iter_mut() {
                let (v, o) = l.overflowing_add(1);
                *l = v;
                if !o {
                    break;
                }
            }
            let mut b1 = [0u8; 32];
            for i in 0..4 {
                b1[i * 8..i * 8 + 8].copy_from_slice(&t1[i].to_le_bytes());
            }
            if d > 1 {
                assert!(!meets_difficulty(&b1, d), "target+1 fails {d}");
                assert!(!hash_le_cmp_target(&b1, &t));
            }
        }
        assert!(meets_difficulty(&[0xff; 32], 1));
        assert!(!meets_difficulty(&[0xff; 32], 2));
        assert!(!meets_difficulty(&[0; 32], 0));
    }

    /// `hash * difficulty < 2^256` with plain 32-bit limb arithmetic as the reference.
    fn reference_meets(hash: &[u8; 32], difficulty: u128) -> bool {
        if difficulty == 0 {
            return false;
        }
        let h: Vec<u64> = hash
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as u64)
            .collect();
        let d: Vec<u64> = (0..4)
            .map(|i| (difficulty >> (32 * i)) as u32 as u64)
            .collect();
        let mut out = [0u64; 12];
        for (i, &a) in h.iter().enumerate() {
            for (j, &b) in d.iter().enumerate() {
                out[i + j] += a * b;
                let mut k = i + j;
                while out[k] >> 32 != 0 {
                    let c = out[k] >> 32;
                    out[k] &= 0xffff_ffff;
                    out[k + 1] += c;
                    k += 1;
                }
            }
        }
        out[8..].iter().all(|&x| x == 0)
    }

    #[test]
    fn meets_difficulty_is_exact_for_all_sizes() {
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..20_000 {
            let mut hash = [0u8; 32];
            for c in hash.chunks_mut(8) {
                c.copy_from_slice(&next().to_le_bytes());
            }
            // Leading zero bytes make passing hashes common at every difficulty size.
            let zeros = (next() % 33) as usize;
            for b in hash.iter_mut().rev().take(zeros) {
                *b = 0;
            }
            let bits = next() % 129;
            let d = if bits == 128 {
                u128::MAX
            } else {
                ((next() as u128) << 64 | next() as u128) & ((1u128 << bits) - 1)
            };
            assert_eq!(
                meets_difficulty(&hash, d),
                reference_meets(&hash, d),
                "hash {hash:02x?} difficulty {d}"
            );
        }
    }

    #[test]
    fn dataset_size_schedule() {
        let p = PowParams::STANDARD;
        assert_eq!(p.dataset_items(0) * 64, 2 << 30);
        assert_eq!(p.dataset_items(1) * 64, (2 << 30) + (4 << 20));
        assert_eq!(p.dataset_items(1_000_000) * 64, 12 << 30);
        assert!(p.dataset_pairs(1_000_000) < u32::MAX);
    }
}
