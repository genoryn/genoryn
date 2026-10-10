# Security policy

GENORYN is experimental software that has **not been independently audited**.
The mainnet launched on 10 October 2026 (release 0.4.0) without an external
audit; mine or hold only what you can afford to lose. Internal engineering
reviews (for example [the October 2026 review](docs/security/audit-2026-10.md)
and [the network-privacy review](docs/security/network-privacy-review-2026-10.md))
are not independent cryptographic or consensus audits.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting if enabled for this repository. If
that option is unavailable, first contact the maintainer through a verified
channel to arrange a private report; do not post seed phrases, wallet files,
onion private keys, Tor control credentials, exploitable details or personal
information in a public issue.

No verified security mailbox or production signing key has been configured.
This repository does not invent either. A future signed production release must
publish authenticated reporting contacts and maintainer keys.

## Scope notes

- **Consensus and cryptography:** dependencies are pinned by `Cargo.lock`. The
  old insecure Orchard circuit is never accepted. Upstream maintenance and audits
  do not certify GENORYN's consensus integration. Read
  [ADR 0001](docs/adr/0001-shielded-protocol.md).
- **Network privacy:** Tor support relies on a Tor daemon the operator runs.
  Application-level leak protections do not replace OS firewall isolation and do
  not defeat traffic correlation or a compromised computer. See
  [network privacy](docs/network-privacy.md) and the
  [threat model](docs/security/threat-model.md).
- **Secrets:** wallet passwords, recovery phrases, onion private keys
  (`onion_v3_private_key`) and Tor control passwords must never appear in logs,
  RPC output or command-line arguments. Report any instance as a vulnerability.
