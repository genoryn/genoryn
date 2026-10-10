# ADR 0010: operator limits without changing consensus

Expose bounded local connection, RPC and mempool admission limits in TOML.
Validate all values before opening databases or listeners. One shared semaphore
covers all RPC routes and both public/admin listeners; a per-route limiter would
not enforce the configured node-wide request budget. Defaults preserve
the existing 16 total/16 inbound/8 outbound connection ceilings, 16 RPC requests,
12 MB JSON body bound and 256 transaction/8 MB mempool. Pending handshakes consume
both the inbound and total permits. Zero may disable one connection direction;
at least one direction must remain enabled.

An operator may lower these ceilings to fit a machine. Lower RPC limits can
reject large submissions to that operator's endpoint; they do not make those
blocks invalid through P2P. Mempool admission and template ordering are also
local policy. Proof/state mutation remains serialized and queues stay bounded.

Consensus maxima for encoded blocks, transactions, actions and transactions per
block cannot be changed by a local configuration file. Increasing them would
fork validity rules and requires a separately reviewed, height-activated protocol
decision. The fixed P2P frame ceiling must carry every consensus-valid block;
it is likewise not an arbitrary per-node validity knob.

Configuration is deliberately not an invitation to advertise production DoS
resistance. Real bounded connection tests, parser campaigns and per-commit
ordinary payment labs are required after transport-policy changes.
