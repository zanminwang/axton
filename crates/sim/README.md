# Simulation

`cargo test -p axton-sim` runs the named scenarios for guarantees L, P, A, D and R and a
quick random run (60 seeds, 120 steps, 3 clients, once with direct writes off and once with
them on; every invariant is checked after every step). `SIM_SEEDS=5000 SIM_STEPS=300 cargo test -p axton-sim --test invariants` is the long
form. A failure prints the seed, the full trace and the minimal trace that still fails.

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
