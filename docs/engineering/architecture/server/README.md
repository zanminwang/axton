# Server

## 1. Introduction and Goals

The Rust server validates the sync context and immutable Batch intent, invokes application Handlers/Loaders through the host contract and persists outcomes with business writes. Publication tracks explicit recipients under record/Stream guards. Current-state cursor pages capture canonical authority without persisted response payloads. Node implements the host and HTTP/WebSocket effects, with no alternate sync policy.

## 5. Building Block View

[Implementation](../../../../crates/server/src/batch_entry.rs) owns this component. [The sync protocol](../protocols/wire.md) owns shared context, delivery and settlement rules.

- [backend-interface](backend-interface.md)
- [batches](batches.md)
- [connection](connection/README.md)
- [engine](engine/README.md)
- [persistence](persistence.md)

## 10. Quality Requirements

Changes must preserve the component boundary and the protocol’s commit/failure rules. The joined native gate `integration/sdk/run-host.sh` exercises the generated client, real HTTP/WebSocket backend and SQLite. Installed-package and mobile evidence are separate [release gates](../../releasing.md#release-gates).
