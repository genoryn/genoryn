// KryptoHash v1 OpenCL kernels (AMD, Intel, NVIDIA). The shared core is
// prepended at runtime from kh_core.h.

__kernel void kh_dataset(__global const uint* cache, uint n, __global uint* dataset, uint start, uint count) {
    uint gid = get_global_id(0);
    if (gid >= count) return;
    uint i = start + gid;
    uint out[16];
    kh_item(cache, n, i, out);
    __global uint* dst = dataset + (ulong)i * 16;
    for (int w = 0; w < 16; w++) dst[w] = out[w];
}

__kernel void kh_search(__global const uint* dataset, uint pairs, __global const uint* header,
                        __global const uint* target, ulong start_nonce, __global uint* results, uint max_results) {
    ulong nonce = start_nonce + (ulong)get_global_id(0);
    uint hdr[8], tgt[8], h[8];
    for (int w = 0; w < 8; w++) { hdr[w] = header[w]; tgt[w] = target[w]; }
    kh_hash(dataset, pairs, hdr, nonce, h);
    if (kh_le_target(h, tgt)) {
        uint slot = atomic_inc(&results[0]);
        if (slot < max_results) {
            results[1 + 2 * slot] = (uint)nonce;
            results[2 + 2 * slot] = (uint)(nonce >> 32);
        }
    }
}
