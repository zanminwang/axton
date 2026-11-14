# Simulation scenarios

Current simulation consists of deterministic protocol-5 scenarios, not the retired generated-action runner. [lib.rs](../../../../crates/sim/src/lib.rs) exports [scenario.rs](../../../../crates/sim/src/scenario.rs) and [rng.rs](../../../../crates/sim/src/rng.rs); the RNG's presence does not establish generated coverage.

| Scenario | Inspected assertions | Boundary |
| --- | --- | --- |
| Receipt/authority order and private settlement across reopen. | [scenario.rs](../../../../crates/sim/tests/scenario.rs) checks exact retry bytes, incomplete work after receipt, durable final completion, final text and five persisted boundaries. | Real SQLite Client; acknowledgements are fixtures. |
| Atomic units, expiry, coverage and settlement dispositions. | [scenario_coverage.rs](../../../../crates/sim/tests/scenario_coverage.rs) checks no progress after incomplete/expired units, transitive same-cursor/dependency components, uniqueness transfer and receipt finalization of undeliverable historical targets. | Pure planning/progress with a map projection, not PostgreSQL or production SQLite apply. |
| Unsupported versions preserve independent outcomes; malformed generic intent is rejected. | [mutation_versions.rs](../../../../crates/sim/tests/mutation_versions.rs) includes server member execution through its persistence seam and real SQLite settlement; malformed JSON, digest, bindings and Query-in-Batch checks remain active admission evidence. | In-memory Host seam, not a real PostgreSQL transaction. |

Run `cargo test -p axton-sim --locked`. The [joined host and capacity gates](../../protocol5-adoption.md) provide separate real-boundary evidence.

Evidence below was inspected on 2026-10-09; these suites were not executed for this documentation change. Prior execution records remain in [history](../../history/pre-protocol5/testing/simulation/scenarios.md). Source inspection supplies neither a new passing result nor a complete-coverage claim.
