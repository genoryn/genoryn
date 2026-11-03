// KryptoHash v1 CUDA kernels. Compiled to PTX (kernels/kryptohash.ptx) with:
//   nvcc -ptx -arch=compute_75 -O3 --use_fast_math -o kernels/kryptohash.ptx kernels/kryptohash.cu
// The PTX is JIT-compiled by the driver for the installed GPU (Turing .. Blackwell).

typedef unsigned int uint;
typedef unsigned long long ulong;
#define KH_FN __device__ __forceinline__
#define KH_G
#define KH_UNROLL _Pragma("unroll 2")
// One 128-byte dataset pair as eight read-only 16-byte loads.
#define KH_LOAD_PAIR(ds, p, d)                                              \
    {                                                                      \
        const uint4* q = (const uint4*)(ds + (ulong)(p) * 32);             \
        _Pragma("unroll") for (int v = 0; v < 8; v++) {                    \
            uint4 x = __ldg(q + v);                                        \
            d[4 * v] = x.x; d[4 * v + 1] = x.y; d[4 * v + 2] = x.z; d[4 * v + 3] = x.w; \
        }                                                                  \
    }
#include "kh_core.h"

extern "C" __global__ void kh_dataset(const uint* __restrict__ cache, uint n, uint* __restrict__ dataset,
                                      uint start, uint count) {
    uint gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= count) return;
    uint i = start + gid;
    uint out[16];
    kh_item(cache, n, i, out);
    uint4* dst = (uint4*)(dataset + (ulong)i * 16);
    dst[0] = make_uint4(out[0], out[1], out[2], out[3]);
    dst[1] = make_uint4(out[4], out[5], out[6], out[7]);
    dst[2] = make_uint4(out[8], out[9], out[10], out[11]);
    dst[3] = make_uint4(out[12], out[13], out[14], out[15]);
}

// header: 8 words, target: 8 words (LE 256-bit), results[0] = count, then (lo, hi) nonce pairs.
extern "C" __global__ void kh_search(const uint* __restrict__ dataset, uint pairs, const uint* __restrict__ header,
                                     const uint* __restrict__ target, ulong start_nonce, uint* results,
                                     uint max_results) {
    ulong nonce = start_nonce + (ulong)(blockIdx.x * blockDim.x + threadIdx.x);
    uint hdr[8], tgt[8], h[8];
    for (int w = 0; w < 8; w++) { hdr[w] = header[w]; tgt[w] = target[w]; }
    kh_hash(dataset, pairs, hdr, nonce, h);
    if (kh_le_target(h, tgt)) {
        uint slot = atomicAdd(&results[0], 1u);
        if (slot < max_results) {
            results[1 + 2 * slot] = (uint)nonce;
            results[2 + 2 * slot] = (uint)(nonce >> 32);
        }
    }
}

// Test helper: full hash of `count` consecutive nonces.
extern "C" __global__ void kh_hash_batch(const uint* __restrict__ dataset, uint pairs, const uint* __restrict__ header,
                                         ulong start_nonce, uint count, uint* out) {
    uint gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= count) return;
    uint hdr[8], h[8];
    for (int w = 0; w < 8; w++) hdr[w] = header[w];
    kh_hash(dataset, pairs, hdr, start_nonce + gid, h);
    for (int w = 0; w < 8; w++) out[gid * 8 + w] = h[w];
}
