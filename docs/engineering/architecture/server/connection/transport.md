# Transport

## 1. Introduction and Goals

The Node HTTP/WebSocket host authenticates requests, applies application admission and submits bytes to Rust. Rust validates discriminator 5 and context before business work. Handshake, Batch, ordinary reads and finite materialization have separate carriers; WebSocket offers complete immutable units and hints. Authentication, infrastructure and typed business refusal remain distinct failure classes.

## 5. Building Block View

[Implementation](../../../../../packages/backend/server/index.mts) owns this component. [Protocol 5](../../protocols/sync.md) owns shared context, delivery and settlement rules.

The host sends a WebSocket control Ping every 30 seconds. A peer that does not
answer before the next heartbeat is terminated through the normal socket-close
path. Control frames do not enter the single subscribe-frame protocol. One timer
belongs to each listening host and stops when that listener closes; admitted
negotiation and database pulls still drain before close resolves.

[Live heartbeat tests](../../../../../integration/persistence/server/live-heartbeat.test.mjs)
exercise real WebSockets through an idle-cut proxy, a peer that stops answering,
timer cleanup and admitted-negotiation drainage. Their native controller is a
fixture; [shutdown tests](../../../../../integration/persistence/server/live-shutdown.test.mjs)
cover the real native/database boundary.
