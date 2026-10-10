# ADR 0009: explicit fee recommendations and miner ordering

Use deterministic public-fee-per-encoded-byte ordering for local block templates,
with transaction ID as the stable tie breaker. Integer cross multiplication
avoids floating-point money arithmetic. All consensus checks, block resource
limits and the minimum fee remain unchanged; a valid block need not follow this
miner policy.

The wallet offers Economy, Standard and Priority recommendations at respectively
1, 2 and 5 times the existing minimum fee. This is an explicit fixed development
policy estimate, not measured congestion, a promised inclusion time or finality.
A custom fee remains available and the payment preview always shows the actual
fee before authorization. Larger encoded transactions can have a lower fee rate
than smaller transactions with the same absolute fee.

No fee service receives the wallet's address, balance or intended recipient.
Dynamic market estimates need their own measured sample policy and adversarial
analysis before replacing these clearly labeled recommendations.
