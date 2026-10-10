# ADR 0005: bounded integer LWMA-style devnet retarget

Every block retargets using up to 60 weighted solve intervals. Interval weights
increase toward the tip. Signed solve times are clamped to +/-360 seconds,
weighted time is floored at 10% of target, and each adjustment is limited to
75%-125% of the previous difficulty. Minimum difficulty is one. Work is the
sum of validated difficulties; ties preserve the current branch.

This is a GENORYN-specific integration inspired by the
[LWMA family](https://github.com/zawy12/difficulty-algorithms), not a claim that
an existing chain's security analysis applies unchanged. Samples under eleven
blocks use the network initial difficulty. Timestamps must exceed the preceding
eleven-block median and may not exceed observed UTC time by 120 seconds.
Local devnet miners therefore need not simulate minute-long waits.

Stable, 10x increase, 90% decrease, oscillation and extreme timestamp/partition
inputs are executable tests. Long-lived adversarial economic simulation and
independent review remain mainnet gates. The target is 60 seconds, not a promise
of immediate deterministic finality.
