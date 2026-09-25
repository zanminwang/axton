# Connection

The connection is how the client reaches a server: the network calls themselves, and the logic that decides when to make them.

- [Transport](transport.md) — Send and receive HTTP/WebSocket messages.
- [Controller](controller/README.md) — Decide when to push, when to stream, when to catch up and when to retry; coordinate subscriptions, cancellation, reconnect and authentication refresh.
  - [Scheduling](controller/scheduling.md) — Per-lane state machine for cycles, retries, pause, resume and close.
  - [Push lane](controller/push-lane.md) — Freeze, send, acknowledge, repeat.
  - [Downlink worker](controller/downlink-worker.md) — Subscribe, catch up over HTTP, queue and commit pages, recover from gaps and subscription changes.
  - [Live session](controller/live-session.md) — One socket attempt of the worker: wire subscription, epoch and handshake order.

## Code map

| Part | Code location |
|---|---|
| Transport | [client-js/transport.mts](../../../../../packages/client-js/transport.mts), [client-js/live.mts](../../../../../packages/client-js/live.mts), [dart/live.dart](../../../../../packages/dart/lib/src/live.dart) |
| Controller / Scheduling | [client/connection.rs](../../../../../crates/client/src/connection.rs); host loops in [client-js/connection.mts](../../../../../packages/client-js/connection.mts) and [dart/connection.dart](../../../../../packages/dart/lib/src/connection.dart) |
| Controller / Push lane | [client/transport.rs](../../../../../crates/client/src/transport.rs) (`SyncCycle`); loops in [client-js/runtime.mts](../../../../../packages/client-js/runtime.mts) and [dart/client.dart](../../../../../packages/dart/lib/src/client.dart) |
| Controller / Downlink worker | [client/downlink_worker.rs](../../../../../crates/client/src/downlink_worker.rs) (`DownlinkWorker`) with the socket session in [client/live.rs](../../../../../crates/client/src/live.rs) (`LiveSession`); dispositions in [client/transport.rs](../../../../../crates/client/src/transport.rs); executors `startDownlinkLane` in [client-js/connection.mts](../../../../../packages/client-js/connection.mts) and `DownlinkLane` in [dart/connection.dart](../../../../../packages/dart/lib/src/connection.dart) |
