# Engine

## 1. Introduction and Goals

The engine admits Store/Stream/context and exact immutable intent before application work. It saves each Mutation result with business effects, publishes in the same Serializable transaction and builds finite cursor pages after preparation closure. Ordinary reads return null-cursor snapshots. Saved Mutation replay does not rerun preparation; cursor reads build fresh responses.

## 5. Building Block View

[Implementation](../../../../../crates/server/src/batch_entry.rs) owns this component. [Protocol 5](../../protocols/wire.md) owns shared context, delivery and settlement rules.

- [publish](publish.md)

## 10. Quality Requirements

Changes must preserve the component boundary and the protocol’s commit/failure rules. The joined native gate `integration/v05-sdk/run-host.sh` exercises the generated client, real HTTP/WebSocket backend and SQLite. Installed-package and mobile evidence are separate adoption gates.
