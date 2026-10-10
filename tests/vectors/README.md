# Consensus vectors

`devnet-block-1.bin` is an actual mined local-development block from the isolated
three-node lab, encoded with the canonical consensus codec. It contains a real Halo 2 coinbase bundle paying the
early 1,000 GNR subsidy (no treasury output before height 501) and genuine RandomX proof of work. There are
no recovery phrases, passwords, private keys or unencrypted private messages.
These are valueless development fixtures, not a mainnet allocation.

The adjacent `.sha256` is the protocol's domain-separated header/block hash,
not the generic SHA256 digest of the entire file. `genoryn-vectors` prints full
header bytes, all roots, genesis identities and the RandomX digest. Linux and
Windows compare against the same checked-in golden JSON. The monetary fixture
exercises all meaningful boundaries using exact atomic strings.

`retired-mainnet-block-494.bin` is a real block (with one private transfer) of the retired
pre-1.0 mainnet, taken from its public block store. Mainnet 1.0 must reject the block and
every transaction in it (replay protection).
