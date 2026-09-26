# Scheduling

## 1. Introduction and Goals

Scheduling answers one question per lane: should the host run a cycle now, wait, or stay idle? Putting that decision in Rust keeps retry policy identical across languages; the host only supplies the clock, entropy, timers and the network call.

## 3. Context and Scope

Interface to Rust for the push lane: events `start`, `stop`, `pause`, `resume`, `wake`, `success`, `failure`, and the query `next` → `idle` | `sync` | `wait {millis}` ([Bindings](../../../sdks/bindings.md), command `connection`). The downlink lane embeds the same driver in `DownlinkWorker`: its `downlink` command takes the same controls and answers `wait {millis}` among its actions ([Downlink worker](downlink-worker.md)). Interface to the application: `Client.connect(server, {onError, refreshAuth})` → `{pause, resume, wake, close}`.

## 5. Building Block View

The Rust `ConnectionDriver` holds six fields: running, paused, dirty, in flight, attempt count and the time the next attempt is due. `next` returns `sync` only when the lane is running, not paused, has nothing in flight and is dirty; `wait` while a retry is due in the future; `idle` otherwise. A successful completion clears the attempt count and leaves the lane clean; a failure marks it dirty and schedules the next attempt at 250 ms doubling per attempt, capped at 30 s, with ±20 % jitter from host entropy. `wake` marks the lane dirty without interrupting a cycle in flight. `restart`, used by the Downlink worker after a replica rebuild, keeps running and paused, forgets the attempt in flight and its backoff, and leaves a running lane dirty and due at once ([Downlink worker](downlink-worker.md)).

The downlink lane applies the same policy to its second work class: a Bootstrap page is scheduled only while the lane is started and not paused, a transport failure defers the next page by the same bounded backoff, and a `resume` clears the deferral. That schedule is independent of the session's - a load is not retried by reopening a socket, and a socket is not reopened by a failed load - and it has no overall timeout, because waiting for connectivity is not a failure ([Downlink worker](downlink-worker.md)).

Both host loops poll `next`, do what Rust asks, and otherwise sleep until the timer fires or a wake arrives. A wake generation counter, bumped by every control or enqueue and re-checked before sleeping, makes a wake that lands while a decision is being made take effect instead of being lost. The difference is the body: the push lane runs one cycle when told to sync and reports the outcome; the downlink lane executes the pump's actions and pumps again while actions come back ([Downlink worker](downlink-worker.md)).

Code: [client/connection.rs](../../../../../../crates/client/src/connection.rs); host loops in [client-js/connection.mts](../../../../../../packages/client-js/connection.mts) (`startConnection`, `startDownlinkLane`) and [dart/connection.dart](../../../../../../packages/dart/lib/src/connection.dart) (`RuntimeConnection`, `DownlinkLane`).

## 6. Runtime View

`pause` aborts the lane's in-flight request, tells Rust to pause, waits for the running body to finish, and wakes the loop so it observes the paused state; a failure caused by the abort is reported as success so no backoff is scheduled. On the downlink lane the host abandons the session's socket and request - and the historical page in flight, which belongs to the lane rather than to the socket - and Rust ends the session without backoff. That abandonment is the lane's own, so it is not reported to the application, but the worker is still told (`failed`), which is how it clears its slot; `resume` clears the deferral and the page is asked for again. While paused no request of either kind starts, and an answer the host had already fetched is still applied: it is local work with no I/O, and its records are stamp-idempotent ([Downlink worker](downlink-worker.md)). `resume` clears the pause and marks the lane dirty. `close` stops the loop, aborts requests and detaches; controls on a closed connection are no-ops, so a stale handle cannot affect a replacement.

On failure the host calls `onError`. If the failure was a 401 (an error with `status: 401` in TypeScript, `AuthenticationExpired` in Dart) and the application supplied `refreshAuth`, it is called before `failure` (push lane) or `closed`/`failed` (downlink lane) is reported; concurrent 401s on both lanes share one refresh. A `failed` event carries the HTTP status the host had, because a Bootstrap run is failed by a refusal the server decided and retried after anything else.

## 10. Quality Requirements

- **Backoff is bounded and a wake never busy-loops or gets lost.** Evidence: unit tests in [client/connection.rs](../../../../../../crates/client/src/connection.rs); [connection.test.mjs](../../../../../../integration/bindings/client-js/connection.test.mjs) `wake arriving during idle decision cannot be lost` and `downlink wake arriving during the idle decision cannot be lost`; [dart/test/connection_test.dart](../../../../../../packages/dart/test/connection_test.dart).
- **Close abandons a network call that never resolves and reports neither success nor failure; closed controls are inert.** Evidence: the remaining tests in those two files.
- **The two lanes have independent lifecycle and retry state.** Evidence: [bindings/common/tests/session.rs](../../../../../../bindings/common/tests/session.rs) `downlink_and_push_drivers_have_independent_lifecycle_and_retry_state`; the downlink lane's backoff, pause, resume and stop in [sqlite/tests/downlink_worker.rs](../../../../../../crates/sqlite/tests/downlink_worker.rs) `a_dropped_socket_reconnects_with_backoff_and_resubscribes`, `pause_ends_the_session_without_backoff_resume_reopens_and_stop_is_final`.
- **A failed auth refresh is retried on the next attempt.** Evidence: [live.test.mjs](../../../../../../integration/bindings/client-js/live.test.mjs) `client retries upgrade authentication and survives failed refresh`; the Dart equivalent in [live_test.dart](../../../../../../packages/dart/test/live_test.dart).

Tests read, not executed.
