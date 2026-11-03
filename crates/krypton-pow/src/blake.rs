//! Minimal BLAKE3 compression used by KryptoHash. Identical to the GPU kernels and
//! checked against the reference `blake3` crate in tests.

pub const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];
const PERM: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
pub const CHUNK_START: u32 = 1;
pub const CHUNK_END: u32 = 2;
pub const ROOT: u32 = 8;

#[inline(always)]
fn g(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(x);
    s[d] = (s[d] ^ s[a]).rotate_right(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(12);
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(y);
    s[d] = (s[d] ^ s[a]).rotate_right(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(7);
}

/// BLAKE3 compression with counter 0; returns the full 16-word output.
#[inline(always)]
pub fn compress(cv: &[u32; 8], block: &[u32; 16], block_len: u32, flags: u32) -> [u32; 16] {
    let mut s = [
        cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7], IV[0], IV[1], IV[2], IV[3], 0, 0,
        block_len, flags,
    ];
    let mut m = *block;
    for r in 0..7 {
        g(&mut s, 0, 4, 8, 12, m[0], m[1]);
        g(&mut s, 1, 5, 9, 13, m[2], m[3]);
        g(&mut s, 2, 6, 10, 14, m[4], m[5]);
        g(&mut s, 3, 7, 11, 15, m[6], m[7]);
        g(&mut s, 0, 5, 10, 15, m[8], m[9]);
        g(&mut s, 1, 6, 11, 12, m[10], m[11]);
        g(&mut s, 2, 7, 8, 13, m[12], m[13]);
        g(&mut s, 3, 4, 9, 14, m[14], m[15]);
        if r < 6 {
            let old = m;
            for i in 0..16 {
                m[i] = old[PERM[i]];
            }
        }
    }
    for i in 0..8 {
        s[i] ^= s[i + 8];
        s[i + 8] ^= cv[i];
    }
    s
}

/// 64-byte BLAKE3 XOF output of a single block of `len` bytes (<= 64).
#[inline(always)]
pub fn b64(block: &[u32; 16], len: u32) -> [u32; 16] {
    compress(&IV, block, len, CHUNK_START | CHUNK_END | ROOT)
}

/// 32-byte BLAKE3 hash of 96 bytes given as 24 words.
#[inline(always)]
pub fn hash96(first: &[u32; 16], second: &[u32; 8]) -> [u32; 8] {
    let o = compress(&IV, first, 64, CHUNK_START);
    let cv: [u32; 8] = o[..8].try_into().expect("8");
    let mut blk = [0u32; 16];
    blk[..8].copy_from_slice(second);
    let o = compress(&cv, &blk, 32, CHUNK_END | ROOT);
    o[..8].try_into().expect("8")
}

pub fn words_to_bytes<const N: usize>(w: &[u32; N]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn bytes_to_words(b: &[u8], out: &mut [u32]) {
    for (i, c) in b.chunks(4).enumerate() {
        let mut x = [0u8; 4];
        x[..c.len()].copy_from_slice(c);
        out[i] = u32::from_le_bytes(x);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_reference_blake3() {
        for len in [0usize, 1, 40, 63, 64] {
            let input: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let mut w = [0u32; 16];
            bytes_to_words(&input, &mut w);
            let mut expect = [0u8; 64];
            blake3::Hasher::new()
                .update(&input)
                .finalize_xof()
                .fill(&mut expect);
            assert_eq!(
                words_to_bytes(&b64(&w, len as u32)),
                expect.to_vec(),
                "len {len}"
            );
        }
        let input: Vec<u8> = (0..96u32).map(|i| (i * 13 + 1) as u8).collect();
        let mut a = [0u32; 16];
        let mut b = [0u32; 8];
        bytes_to_words(&input[..64], &mut a);
        bytes_to_words(&input[64..], &mut b);
        assert_eq!(
            words_to_bytes(&hash96(&a, &b)),
            blake3::hash(&input).as_bytes().to_vec()
        );
    }
}
