/* KryptoHash v1 core shared by the CUDA and OpenCL kernels.
 * Must stay bit-for-bit identical to crates/krypton-pow/src/lib.rs.
 * Expects: uint (32-bit), ulong (64-bit), KH_FN, KH_G (global pointer qualifier). */

#define KH_ACCESSES 64
#define KH_PARENTS 64
#define KH_FNV 0x01000193u
#define KH_TWEAK 0x9E3779B9u

KH_FN uint kh_fnv(uint a, uint b) { return (a * KH_FNV) ^ b; }
KH_FN uint kh_rotr(uint x, uint n) { return (x >> n) | (x << (32u - n)); }
KH_FN uint kh_rotl(uint x, uint n) { return (x << n) | (x >> (32u - n)); }

#define KH_G_(a, b, c, d, x, y)              \
    a = a + b + (x); d = kh_rotr(d ^ a, 16); \
    c = c + d;       b = kh_rotr(b ^ c, 12); \
    a = a + b + (y); d = kh_rotr(d ^ a, 8);  \
    c = c + d;       b = kh_rotr(b ^ c, 7);

#define KH_ROUND(m)                                         \
    KH_G_(s0, s4, s8, s12, m[0], m[1]);                     \
    KH_G_(s1, s5, s9, s13, m[2], m[3]);                     \
    KH_G_(s2, s6, s10, s14, m[4], m[5]);                    \
    KH_G_(s3, s7, s11, s15, m[6], m[7]);                    \
    KH_G_(s0, s5, s10, s15, m[8], m[9]);                    \
    KH_G_(s1, s6, s11, s12, m[10], m[11]);                  \
    KH_G_(s2, s7, s8, s13, m[12], m[13]);                   \
    KH_G_(s3, s4, s9, s14, m[14], m[15]);

#define KH_PERMUTE(m)                                                                       \
    {                                                                                       \
        uint t0 = m[0], t1 = m[1], t2 = m[2], t3 = m[3], t4 = m[4], t5 = m[5], t6 = m[6], \
             t7 = m[7], t8 = m[8], t9 = m[9], t10 = m[10], t11 = m[11], t12 = m[12],        \
             t13 = m[13], t14 = m[14], t15 = m[15];                                         \
        m[0] = t2; m[1] = t6; m[2] = t3; m[3] = t10; m[4] = t7; m[5] = t0; m[6] = t4;       \
        m[7] = t13; m[8] = t1; m[9] = t11; m[10] = t12; m[11] = t5; m[12] = t9;             \
        m[13] = t14; m[14] = t15; m[15] = t8;                                               \
    }

/* BLAKE3 compression, counter 0. cv: 8 words, m: 16 words (modified), out: 16 words. */
KH_FN void kh_compress(const uint* cv, uint* m, uint block_len, uint flags, uint* out) {
    uint s0 = cv[0], s1 = cv[1], s2 = cv[2], s3 = cv[3], s4 = cv[4], s5 = cv[5], s6 = cv[6], s7 = cv[7];
    uint s8 = 0x6A09E667u, s9 = 0xBB67AE85u, s10 = 0x3C6EF372u, s11 = 0xA54FF53Au;
    uint s12 = 0, s13 = 0, s14 = block_len, s15 = flags;
    KH_ROUND(m); KH_PERMUTE(m);
    KH_ROUND(m); KH_PERMUTE(m);
    KH_ROUND(m); KH_PERMUTE(m);
    KH_ROUND(m); KH_PERMUTE(m);
    KH_ROUND(m); KH_PERMUTE(m);
    KH_ROUND(m); KH_PERMUTE(m);
    KH_ROUND(m);
    out[0] = s0 ^ s8; out[1] = s1 ^ s9; out[2] = s2 ^ s10; out[3] = s3 ^ s11;
    out[4] = s4 ^ s12; out[5] = s5 ^ s13; out[6] = s6 ^ s14; out[7] = s7 ^ s15;
    out[8] = s8 ^ cv[0]; out[9] = s9 ^ cv[1]; out[10] = s10 ^ cv[2]; out[11] = s11 ^ cv[3];
    out[12] = s12 ^ cv[4]; out[13] = s13 ^ cv[5]; out[14] = s14 ^ cv[6]; out[15] = s15 ^ cv[7];
}

KH_FN void kh_iv(uint* cv) {
    cv[0] = 0x6A09E667u; cv[1] = 0xBB67AE85u; cv[2] = 0x3C6EF372u; cv[3] = 0xA54FF53Au;
    cv[4] = 0x510E527Fu; cv[5] = 0x9B05688Cu; cv[6] = 0x1F83D9ABu; cv[7] = 0x5BE0CD19u;
}

/* 64-byte XOF output of one block of `len` bytes: flags CHUNK_START|CHUNK_END|ROOT = 11. */
KH_FN void kh_b64(uint* m, uint len, uint* out) {
    uint cv[8];
    kh_iv(cv);
    kh_compress(cv, m, len, 11u, out);
}

/* Dataset item i from the cache (n items of 16 words). */
KH_FN void kh_item(const KH_G uint* cache, uint n, uint i, uint* out) {
    uint mix[16];
    const KH_G uint* c = cache + (ulong)(i % n) * 16;
    for (int w = 0; w < 16; w++) mix[w] = c[w];
    mix[0] ^= i;
    uint t[16];
    kh_b64(mix, 64u, t);
    for (uint j = 0; j < KH_PARENTS; j++) {
        uint p = kh_fnv(i ^ j, t[j & 15]) % n;
        const KH_G uint* cp = cache + (ulong)p * 16;
        for (int w = 0; w < 16; w++) t[w] = kh_fnv(t[w], cp[w]);
    }
    kh_b64(t, 64u, out);
}

/* Full hash of (header, nonce) over the dataset; writes 8 output words. */
KH_FN void kh_hash(const KH_G uint* ds, uint pairs, const uint* hdr, ulong nonce, uint* out) {
    uint blk[16];
    for (int w = 0; w < 8; w++) blk[w] = hdr[w];
    blk[8] = (uint)nonce;
    blk[9] = (uint)(nonce >> 32);
    for (int w = 10; w < 16; w++) blk[w] = 0;
    uint s[16];
    kh_b64(blk, 40u, s);
    uint mix[32];
    for (int w = 0; w < 16; w++) { mix[w] = s[w]; mix[w + 16] = s[w] ^ KH_TWEAK; }
#ifdef KH_UNROLL
    KH_UNROLL
#endif
    for (uint a = 0; a < KH_ACCESSES; a++) {
        uint p = kh_fnv(a ^ s[0], mix[a & 31]) % pairs;
        uint rot = (a & 15) + 1;
#ifdef KH_LOAD_PAIR
        uint d[32];
        KH_LOAD_PAIR(ds, p, d);
#else
        const KH_G uint* d = ds + (ulong)p * 32;
#endif
        for (int w = 0; w < 32; w++) mix[w] = kh_rotl(kh_fnv(mix[w], d[w]), rot);
    }
    uint c[16];
    for (int k = 0; k < 8; k++)
        c[k] = kh_fnv(kh_fnv(kh_fnv(mix[4 * k], mix[4 * k + 1]), mix[4 * k + 2]), mix[4 * k + 3]);
    for (int k = 8; k < 16; k++) c[k] = 0;
    uint cv[8], o[16];
    kh_iv(cv);
    kh_compress(cv, s, 64u, 1u, o);
    for (int w = 0; w < 8; w++) cv[w] = o[w];
    kh_compress(cv, c, 32u, 10u, o);
    for (int w = 0; w < 8; w++) out[w] = o[w];
}

/* Little-endian 256-bit comparison: hash <= target. */
KH_FN int kh_le_target(const uint* h, const uint* t) {
    for (int i = 7; i >= 0; i--) {
        if (h[i] != t[i]) return h[i] < t[i];
    }
    return 1;
}
