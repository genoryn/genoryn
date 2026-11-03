# Integrating Genoryn Krypton

## Exchanges and payment processors

1. Run your own `kryptond` (full node). Do not rely on third-party nodes for deposits.
2. Create one wallet per service with `krypton-wallet-cli new`; store the recovery phrase offline.
3. Give each customer a distinct memo or a distinct wallet account. All transfers are shielded:
   deposits are detected by trial decryption in your wallet, not by watching addresses.
4. Credit deposits after **10 confirmations** (more for large amounts); mining payout notes
   mature after 100 blocks.
   Mining pools are unnecessary: the protocol pays every miner in proportion to its shares
   ([rewards.md](../spec/rewards.md)). `GET /v1/miner/{address}` shows a miner's unpaid
   balance and next payout height; `GET /v1/shares` the shares waiting to be credited.
5. Withdrawals: `krypton-wallet-cli send <address> <amount> --memo <ref>`. A transaction
   spends at most two notes; the wallet automatically consolidates in parallel rounds if needed.
6. Fees: the default 0.002 GNR is well above the relay minimum (1,000 gori per KB, ~0.001 GNR).

## Node API

REST (`/v1`), JSON-RPC 2.0 (`/rpc`) and WebSocket (`/v1/ws`). See `apps/kryptond/src/rpc.rs`.

```sh
curl -s localhost:31334/v1/info
curl -s localhost:31334/rpc -d '{"jsonrpc":"2.0","id":1,"method":"getblock","params":[100]}'
curl -s localhost:31334/v1/validate/<address>
```

## Wallets

Embed `krypton-wallet-core` (Rust) and implement `ChainSource` or use the provided HTTP
`NodeClient`. Keys, scanning and proving stay on the device. For other languages use the C ABI
(`crates/krypton-sdk/include/krypton.h`) for address validation and amounts, and the node API.

## Explorers

Use `/v1/blocks`, `/v1/block/{id}`, `/v1/tx/{txid}`, `/v1/nullifier/{hex}` and the WebSocket
feed. Amounts and parties of transfers are not public by design; show fees, sizes and proofs.

## Mining pools

`POST /v1/mining/template {address}` returns a job (`pow_hash`, `difficulty`, `target`,
`epoch`, `chain_seed`); `POST /v1/mining/submit {job_id, nonce}` submits. A pool can hand
out the same job with smaller share targets and submit full solutions.

## Atomic swaps and bridges (roadmap)

Hash-time-locked notes (a hash preimage and a height bound inside the spend circuit) are the
planned primitive for trust-minimized swaps; they will arrive as a versioned circuit by
soft fork.
