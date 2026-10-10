# Wallet architecture

`genoryn-wallet-core` owns seed/key derivation, encrypted files, receiver encoding,
full-chain validation and scanning, balance/history reconstruction, transaction
proof/signature construction, payment disclosure, username signing and release
verification. The native GUI does not duplicate consensus logic.

The GUI invokes the sibling wallet CLI with one bounded JSON request on standard
input and one bounded response on standard output. Passwords and recovery phrases
never enter process arguments. Generation IDs reject late asynchronous responses;
locking clears private views and secret buffers and cancels owned CLI workers.
Node/miner lifecycle ownership prevents the GUI from stopping unrelated processes.

A 24-word BIP39 seed comes from OS randomness. ZIP32 derives Orchard spending
material under an explicitly experimental GENORYN/network domain; HKDF derives a
separate username signing identity. These are established primitives, not a new
mnemonic scheme. The CLI can derive additional ZIP32 accounts and diversified
receivers. Recovery uses the same seed plus the account index; encrypted wallet
files retain that index. Scanning recognizes all receivers of the selected account.
Local labels, owner pins and resolver records require separate backup.

Wallet files use Argon2id (64 MiB,3 passes,1 lane) and XChaCha20-Poly1305 with fresh
salt/nonce and authenticated version/network metadata. Unix permissions are 0600;
directories are private. Windows file access inherits the user's profile ACL;
DPAPI integration is not claimed. Memory zeroization is applied to supported key
types and application secret buffers, not a guarantee against a compromised OS.

Full local rescan validates block work/proofs/state before balance reconstruction.
The confirmation display threshold is configurable in native Settings or with
`genoryn-wallet-cli --wallet .devnet/alice.wallet --confirmation-target 20 balance`;
it does not change
consensus maturity or imply deterministic finality. Choose the threshold for the
payment's value and tolerated reorganization risk.
This conservative v1 path is expensive for long histories. Pruning, efficient
incremental wallet databases, hardware signing, watch-only export and private
light-client protocols are roadmap items. Never add an address-indexed balance RPC
as a shortcut.

## Accounts and derived receivers

```sh
# A new encrypted file for account 7; existing files are never overwritten.
genoryn-wallet-cli --wallet alice.wallet derive-account 7 alice-account7.wallet
# Restore that account from the same 24-word seed, entered through a hidden prompt.
genoryn-wallet-cli --wallet restored-account7.wallet --account 7 restore
# A distinct receiver from the selected account; no seed or private key is printed.
genoryn-wallet-cli --wallet alice-account7.wallet address --index 42
```

Account indices are 0 through 2^31−1; address indices are 0 through 2^32−1.
Keep the account numbers with your offline recovery instructions. The software
does not scan billions of possible accounts to guess which ones were used.
Different accounts share the seed-derived username signing identity, while
spending/viewing keys and receivers are distinct. Exported payment proofs reveal
only selected outputs; a full watch-only wallet remains a separate roadmap item.

## Fee policy

```sh
genoryn-wallet-cli estimate-fee economy
genoryn-wallet-cli estimate-fee standard
genoryn-wallet-cli estimate-fee priority
```

These local, key-free quotes use 1×, 2× and 5× the existing minimum fee. They
are fixed development recommendations, not congestion measurements or promised
confirmation times. Templates prioritize the public fee per encoded byte.
The native Send view offers the same quotes plus an editable exact fee; the
confirmation preview always uses that actual fee. See [ADR 0009](../adr/0009-fee-policy.md).
