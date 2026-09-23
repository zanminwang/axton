# Simulation

Simulation is a testing method and environment. AXTON uses it to exercise the real Rust client and server together through a controlled network. Clients use temporary SQLite files; the server uses an in-memory host. This makes message ordering and restarts reproducible without running a network server or PostgreSQL.

- [Scenarios](scenarios.md) — Named examples of overall behavior.
- [Invariants](invariants.md) — Checks across generated sequences.
- [Failure and recovery](recovery.md) — Fault injection, replay and shrinking.

Implementation: [crates/sim](../../../../crates/sim). Behavioral contract: [guarantees](../../guarantees.md). The harness does not establish real PostgreSQL isolation, FFI lifetimes or socket behavior; those need [Integration tests](../integration/README.md).

```sh
cargo test -p axton-sim --locked
```
