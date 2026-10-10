# Versioning and upgrades

Current versions: consensus 1, block 1, transaction 1, P2P1, RPC`/v1`, wallet-file 1,
database schema 1, receiver-bundle 1 and experimental payment-URI1. Unsupported
versions fail closed. Networks have distinct genesis, magic, ports and receiver
prefixes; database reuse across networks is rejected.

Future consensus upgrades must publish a specification and activation height,
review new proof/verifying keys and monetary invariants, generate cross-platform
vectors, and require explicit compatible software. No automatic founder override
or remotely signed financial exception exists. The code currently supports only
v1, so it cannot silently guess another version.

Mainnet genesis is not frozen. Dependency/serialization changes can invalidate
development databases and must be documented before any production freeze.
No software update can be silently executed: release verification uses an explicit
trusted public key and the user decides whether to install. Independent builder
reproducibility, production signing and multi-maintainer policy remain gates.
