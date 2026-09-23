# Push lane

## 1. Introduction and Goals

The push lane moves frozen batches to the server and their receipts back into the engine. It never pulls: authoritative content arrives through the [live session](live-session.md).

## 3. Context and Scope

Rust commands: `startSync {pushOnly: true}`, then `next` → `{kind: "push", body}` or `null`, then `complete {response}` → the reports of what the receipt could not apply. Network: `POST /sync/mutations` through the [transport](../transport.md). The lane runs whenever [scheduling](scheduling.md) says `sync`.

## 5. Building Block View

The Rust `SyncCycle` remembers one active action so a request that failed is retried with the same bytes. In push-only mode `next` asks the engine to freeze; a batch already in flight comes back unchanged ([Batching](../../engine/push/batching.md)). `complete` decodes the receipt and acknowledges it, which completes the batch with the server's content ([Settlement](../../engine/settlement.md)). The cycle's full mode, which also issues HTTP pulls per subscribed channel, is retained for tests and the internal protocol fixture and is not reachable from `connect`.

Code: [client/transport.rs](../../../../../../crates/client/src/transport.rs) (`SyncCycle`); the loop in `#runSync` in [client-js/runtime.mts](../../../../../../packages/client-js/runtime.mts) and `_runSync` in [dart/client.dart](../../../../../../packages/dart/lib/src/client.dart).

## 6. Runtime View

One cycle: restart the push-only cycle, loop `next` → send → `complete` until `next` returns `null`, then report success. A transport error ends the cycle with a failure; the frozen batch stays in flight and the next cycle resends it. A receipt completes its batch at once; nothing waits for the live lane. A receipt the engine refuses (another client or batch, or missing authority) is a cycle failure: the batch stays frozen and is resent. A receipt record the engine cannot apply is not: it is skipped, the batch completes, and the SDK hands each report `complete` returned (skipped, conflict, diverged) to `onError` as an `AxtonReport`.

## 10. Quality Requirements

- **A retried push reuses the frozen request; a receipt completes the push without a pull and the visible row is the server's; a page carrying the same authority later is a no-op that advances the cursor; the lane never issues a pull.** Evidence: [bindings/common/tests/session.rs](../../../../../../bindings/common/tests/session.rs) `rust_selects_transport_actions_and_reuses_frozen_request_on_retry`, `live_push_cycle_keeps_receipts_but_leaves_reads_to_the_stream`; [live.test.mjs](../../../../../../integration/bindings/client-js/live.test.mjs) `a receipt record the client cannot apply reaches onError and the batch still completes`; [live_test.dart](../../../../../../packages/dart/test/live_test.dart) `what a page cannot apply reaches onError as an AxtonReport…` (its last step covers a receipt).
- **A client with no subscribed channels still pushes.** Evidence: [live.test.mjs](../../../../../../integration/bindings/client-js/live.test.mjs) `a reusable server config isolates cancellation and no-channel clients only push`.

Executed 2026-09-15: `cargo test -p axton-binding --locked` passed with the session tests above; `live.test.mjs` read, not executed.
