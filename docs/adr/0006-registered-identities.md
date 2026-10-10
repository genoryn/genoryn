# ADR 0006: registered-identity capacity without changing consensus

Accepted for the local resolver after the user's clarification: tens of thousands
of registered identities, not concurrent payments. A wallet is a local key set,
not an account registered with a central chain server. Public username ownership
is a separate optional resolver subsystem.

The original 10,000-entry JSON registry limit and whole-file rewrite per update
were provisional implementation choices. Replace daemon persistence with an
indexed SQLite table, bounded to 100,000 signed receiver records. Each mutation
uses an immediate atomic transaction, checks existing ownership/sequence and the
shared database record count, and updates only one row. Name lookup is indexed;
the complete registry is never materialized for an ordinary RPC request.

Existing JSON registries migrate once, with signature/network validation, in one
transaction. The original file remains a backup. Schema/network mismatches fail
closed. Database integrity is checked on startup; expired records retain ownership
for authenticated renewal. Resource limits and public first-use trust limitations
remain explicit. The resolver is still local/off-chain, not globally decentralized.

Validate with a 50,000-distinct-wallet/owner/name dataset, persistence/reopen,
indexed lookup, renewal, and recorded runtime/disk/resource measurements. This
does not claim 50,000 concurrent RPC clients or transactions per second.

The 60-second block target is user-specified. The 64-transfer block limit is a
provisional consensus resource cap, implying at most about 1.07 transfers/second
at that target before further constraints. Neither is changed for registered-user
capacity. Any throughput retuning needs separate consensus review, measured proof
and propagation costs, state-growth analysis and an activation specification.
