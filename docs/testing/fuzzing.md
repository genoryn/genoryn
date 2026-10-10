# Parser fuzzing

The normal stable test suite runs 512-case arbitrary-byte and mutated real-block
property tests, checking bounded parsing, canonical roundtrips and hostile vector
lengths. P2P tests separately exercise incorrect network magic and oversize frames;
native-wallet tests reject malformed/ambiguous payment URIs.

Long-running coverage-guided fuzz targets are in `fuzz/`:

```sh
cargo install cargo-fuzz --locked
rustup toolchain install nightly --profile minimal
cargo +nightly fuzz run block_parser -- -max_total_time=300 -rss_limit_mb=2048
cargo +nightly fuzz run transaction_parser -- -max_total_time=300 -rss_limit_mb=2048
cargo +nightly fuzz run username_parser -- -max_total_time=300 -rss_limit_mb=2048
```

These targets do not mine, contact peers or publish anything. The transaction
target covers proof/point/signature deserialization, not expensive proof creation.
Archive minimized crashes before changing a target. Coverage-guided runs are
separate from property testing and must be reported with actual duration/corpus;
the presence of a fuzz target alone is not a completed fuzz campaign.
