/* libkrypton: C interface to Genoryn Krypton (MIT OR Apache-2.0).
 * Build: cargo build --release -p krypton-sdk  ->  target/release/krypton.{dll,so,dylib} and libkrypton.a
 * All strings are NUL-terminated UTF-8. Output buffers are owned by the caller.
 * Functions writing strings return the length written, or -(required size) if `cap` is too small. */
#ifndef KRYPTON_H
#define KRYPTON_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define KRYPTON_COIN 100000000ULL          /* 1 GNR in base units (gori) */
#define KRYPTON_NET_MAINNET 1
#define KRYPTON_NET_TESTNET 2
#define KRYPTON_NET_DEVNET 3

const char *krypton_version(void);
/* Network of a full address, or 0 if invalid (checksum, encoding, key). */
int krypton_address_network(const char *address);
/* Short content-addressed form (gnrs1..., tgnrs1...). */
int krypton_short_address(const char *address, char *out, size_t cap);
/* "12.5" -> 1250000000. Returns 1 on success, 0 on error. */
int krypton_parse_amount(const char *text, uint64_t *out);
int krypton_format_amount(uint64_t gori, char *out, size_t cap);
uint64_t krypton_block_subsidy(uint64_t height);

#ifdef __cplusplus
}
#endif
#endif
