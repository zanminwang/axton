# Simulation

`cargo test -p axton-sim` runs deterministic protocol-5 scenarios over real SQLite: receipts,
authority, delivery and reopen at every persisted boundary. After every step, each Model row
equals the rebuild of its key from its base, its pending operations and its accepted writes.

See the [simulation testing guide](../../docs/engineering/testing/simulation/README.md) and [guarantees](../../docs/engineering/guarantees.md).

## Capacity diagnostic

`cargo run -p axton-sim --example capacity --release` enqueues 10 and 1,000 updates to one
record with one real SQLite commit per mutation, then applies an authoritative page and
checks that pending replay preserves the latest local value. It reports enqueue p50/p95 and
one page-plus-replay duration. It is a diagnostic, not a gate; see issue #12.

Measured 2026-09-10 on the development macOS arm64 host, optimized build, on the
document-store layout that predates #9; re-measure under #12:

| Pending | Enqueue p50 | Enqueue p95 | One page + replay |
| --- | --- | --- | --- |
| 10 | 0.42 ms | 0.67 ms | 0.59 ms |
| 1,000 | 2.17 ms | 3.51 ms | 5.38 ms |

These are local diagnostic samples from a single run with a tiny working set. They do not
measure a large multi-record cache, mobile devices, network latency, bridge overhead or
production throughput. Replay is Rust-only; there are no per-field language callbacks in
this measurement.
