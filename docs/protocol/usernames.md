# On-chain usernames v1

A username is a short public name, such as `@max`, that points to a receiving address.
Names live on the blockchain: every upgraded node derives the same name table from the
same chain, the first valid registration wins, and anyone can check who owns a name.
Payments to a name stay as private as any other payment.

## Rules

- **Names:** 3–32 characters, lowercase ASCII letters, digits and underscores, starting
  with a letter. A leading `@` and upper case are accepted and normalized. Reserved:
  `admin`, `administrator`, `genoryn`, `support`, `security`, `treasury`.
- **Price, burned forever:** 3–4 characters 1,000 GNR; 5–6 characters 100 GNR;
  7 or more 10 GNR. Updating the receiver of an owned name burns 1 GNR. Nobody receives
  the price, so there is no registrar to pay and squatting has a real cost.
- **Permanent:** a registered name never expires and cannot be taken over.
- **Ownership:** the owner is an Ed25519 key derived from the wallet's recovery seed
  (`GENORYN/username/v1`), so restoring the 24 words restores control of the name.
- **First come, first served:** operations apply in chain order (block, transaction,
  output). A registration for a name that already exists, or that burns less than the
  price, has no effect; its burned value is not refunded. Wallets therefore check the
  name, including pending registrations in the mempool, before registering.

## Encoding

A registration is an ordinary private transfer (kind 0) with one extra output:

1. the output pays at least the price to the network's **burn address**, and
2. its 512-byte memo carries the signed record:

| Field | Bytes |
|---|---|
| Magic `GNRNAME\x01` | 8 |
| Operation (`1` register, `2` update) | 1 |
| Name length, then name | 1 + 3…32 |
| Receiver (raw Orchard address) | 43 |
| Owner Ed25519 public key | 32 |
| Sequence (0 for register, previous + 1 for update) | 4 |
| Ed25519 signature | 64 |
| Zero padding | rest |

The owner signs `GENORYN/name-record/v1 ‖ genesis ‖ op ‖ len ‖ name ‖ receiver ‖ owner ‖
sequence`, binding the record to one network.

### The burn address

The burn address belongs to a full viewing key built from public values: `ak` is a
hash-to-curve point (`GENORYN-name-burn-v1`) whose discrete logarithm nobody knows, and
`nk`/`rivk` are public hashes. Without the discrete logarithm of `ak` no spend
authorization signature can exist, so value paid there can never be spent. Because the
viewing key is public, every node decrypts burn outputs; note decryption is checked
against the output's on-chain commitment, so the burned value read by the node is the
value actually committed in the transaction.

## Compatibility and consensus

Names are **derived state**. They never make a block invalid and are not committed in
block headers, so nodes without name support accept exactly the same blocks; a
registration looks to them like any other private transfer. No activation height or
network split is needed. A node with name support computes the table while applying
each fully validated block, rebuilds it when it replays its stored chain at startup,
and recomputes it on reorganizations together with the rest of the chain state.

## Privacy

Public: that a name exists, its receiving address, its owner key, the block of its
registration and the burned amount. Private, exactly as before: who pays a name, how
much, the payer's and owner's balances, and every other address of the owner's wallet.
The wallet points names at its second receiving address (index 1), so publishing a name
does not publish the main receiving identity. Looking up a name asks only the user's own
node; no outside resolver learns whom the user is about to pay.

## Interfaces

- Node: `GET /v1/names?name=` (registration, pending registration, price, availability)
  and `GET /v1/names/owner?owner=<hex>` (names of one owner), on the admin and public
  read-only listeners. See [RPC v1](../rpc/api.md).
- Wallet CLI: `name_status`, `register_name`, `update_name`, `my_names`; `send` to
  `@name` resolves through the chain.
- Desktop wallet: Receive → Username shows availability and price as you type, your
  names, and a confirmation before anything is burned. Send shows what `@name`
  resolves to.

## The earlier local registry

Before on-chain names, a node kept signed receiver bundles in its own `usernames.sqlite`.
That registry still works for compatibility (`/v1/usernames`), and `send` falls back to
it only when the chain has no such name. It is not globally unique and new wallets do
not use it to register names.
