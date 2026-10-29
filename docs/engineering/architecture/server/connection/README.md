# Connection

- [Transport](transport.md) — Send and receive HTTP/WebSocket messages.
- [Controller](controller.md) — Manage WebSocket subscriptions and stream pages as commits arrive.

## Code map

| Part | Code location |
|---|---|
| Transport | [HTTP routing](../../../../../packages/backend/server/connection/http.mts), [WebSocket transport](../../../../../packages/backend/server/connection/live.mts), [shared admission](../../../../../packages/backend/server/connection/admission.mts) and [listener lifecycle](../../../../../packages/backend/server/connection/listener.mts) |
| Controller | [server/live.rs](../../../../../crates/server/src/live.rs) (`Subscriptions`); executor `serveLive` in [connection/live.mts](../../../../../packages/backend/server/connection/live.mts) |
