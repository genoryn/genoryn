# genoryn-connect

A native async Rust discovery and read-only failover SDK. No central master API
or hardcoded production endpoint is shipped. Configure explicit bootstrap
origins and pin the intended network, chain ID and genesis hash independently.
A seed or RPC node does not acquire consensus authority by being discovered.

```rust,no_run
use genoryn_connect::{Connect, ConnectConfig};
# async fn demo() -> anyhow::Result<()> {
let cfg = ConnectConfig::devnet(
    "genoryn-devnet-v1",
    "<trusted 64-character hex genesis hash>",
    vec!["http://127.0.0.1:29411".into()],
);
let mut client = Connect::new(cfg)?;
let node = client.select().await?;
let supply: serde_json::Value = client.get_json("/v1/supply").await?;
# Ok(()) }
```

The caller's pinned genesis must come from trusted configuration/shared network
parameters; never copy a response's identity into an expected-identity setting.

## Discovery and selection

- Maximum 32 candidate origins and 8 explicitly configured DNS TXT names.
- Native system DNS resolves `genoryn-rpc-v1=https://host[:port]` TXT records.
  TXT discovery is an unsigned hint, not a DNSSEC or consensus guarantee. Call
  `discover_dns()` explicitly and inspect its source errors before `select()`.
- `add_manifest` verifies a caller-pinned Ed25519 key, chain identity, version,
  issue/expiry times and a maximum 30-day validity window before atomically
  adding endpoints. There is no implicit key trust-on-first-use.
- HTTPS is required for every non-loopback endpoint. HTTP is accepted only for
  literal loopback IPs, on any network; `localhost` is rejected to avoid
  treating a mutable DNS name as a loopback security boundary. URL credentials,
  non-root paths, query strings, fragments and redirects are rejected.
- GET `/v1/info` probes run in parallel with bounded timeout/body size. Each must
  report the pinned identity, RPC protocol 1 and a valid best-hash encoding.
  Non-loopback nodes must explicitly report `public_rpc: true`.
- Heights outside configured skew from the lower median are rejected, including
  implausibly far-ahead nodes. The remaining endpoints are sorted by observed
  latency; equal latency prefers higher height. With only two disagreeing nodes
  this policy conservatively prefers the lower height rather than trusting an
  unsupported high claim. Set skew for the network's expected propagation lag.
- `get_json` refreshes health, rechecks each selected identity and tries healthy
  candidates until one read succeeds. Only `/v1/info`, `/v1/supply`, and bounded
  parameter-form `/v1/blocks?from=N&limit=N` routes are admitted. Administrative
  and state-mutating requests are intentionally absent.
- `last_report` exposes healthy endpoints and explicit rejection reasons.

This is not Byzantine consensus or an anti-eclipse proof. A malicious majority
can agree on fabricated reports; a single configured node cannot be cross-
checked. A full node must independently validate headers, work and chain state.
The SDK does not send wallet addresses or view/spend keys to a remote service.
Native DNS is not browser-compatible; a future browser transport needs HTTPS
manifests or an explicit trusted browser discovery mechanism and CORS.

## Signed manifest format

`SignedManifest` is JSON with `payload` and lowercase/uppercase hex `signature`.
The Ed25519 signature covers the ASCII bytes `GENORYN bootstrap manifest v1`,
then a single NUL byte, then compact UTF-8 JSON produced in this exact field
order (no extra fields):

```json
{"version":1,"network":"devnet","chain_id":"genoryn-devnet-v1","genesis_hash":"…","issued_at":1,"expires_at":2,"endpoints":["http://127.0.0.1:29411"]}
```

Use `BootstrapManifest::signing_bytes()` instead of independently guessing a
serializer. The trusted verifying key comes from application/operator config,
not the signed document. Test keys in unit tests are fixed, public test vectors
only and are never deployed trust anchors. Rollback protection against an older
still-valid manifest is not persistent in v1; production clients need durable
version/epoch tracking and a reviewed trust-anchor rotation procedure.

Run `cargo test -p genoryn-connect`. Tests use real loopback HTTP listeners for
identity/height rejection and failover, plus manifest tampering/expiry and URL
transport-policy tests. No deployed DNS or production service is required.

For an actual devnet smoke test with multiple independent nodes:

```sh
cargo run -p genoryn-connect --example probe -- \
  genoryn-devnet-v1 TRUSTED_GENESIS_HASH \
  http://127.0.0.1:29411 http://127.0.0.1:29421 http://127.0.0.1:29431
```

The example prints public diagnostics and supply only. Shut down one endpoint
and rerun to verify that another healthy candidate is selected.
