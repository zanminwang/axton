# Storage

## 1. Introduction and Goals

The storage adapter executes SQL and transactions; Rust owns reconciliation policy. Format-5 admission and physical ownership precede schema coordination. Store metadata retains context, S/B/C, frozen Batch, operation input and acknowledgment. Model tables expose the replayed projection. Production SQL reads the canonical Store and Mutation queue/operation tables directly; there are no legacy table-name views or bridge triggers.

## 5. Building Block View

[Implementation](../../../../../crates/client/src/store_file.rs) owns this component. [The sync protocol](../../protocols/wire.md) owns shared context, delivery and settlement rules.

- [reconciliation](reconciliation.md)
- [store](store.md)
- [store-file](store-file.md)

## 10. Quality Requirements

Changes must preserve the component boundary and the protocol’s commit/failure rules. The joined native gate `integration/sdk/run-host.sh` exercises the generated client, real HTTP/WebSocket backend and SQLite. Installed-package and mobile evidence are separate [release gates](../../../releasing.md#release-gates).
