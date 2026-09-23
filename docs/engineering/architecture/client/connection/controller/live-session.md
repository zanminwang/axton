# Live session

## 1. Introduction and Goals

A live session is one attempt to follow the client's channels: open the WebSocket, subscribe, compare the acknowledged heads with the durable cursors, catch up over HTTP only when behind, then consume the stream until something invalidates the session. HTTP fills gaps; the WebSocket carries only what is new. Every page, streamed or fetched, goes through the same gate in [Pull](../../engine/pull.md).

## 3. Context and Scope

The session is a Rust state machine, `LiveSession`, driven the way [scheduling](scheduling.md) and the [push lane](push-lane.md) are: the host feeds events and executes the actions Rust answers with. One binding command, `live`, carries both ([Bindings](../../../sdks/bindings.md)). The host owns the socket, the HTTP requests, the timers, its frame buffer and the credential refresh; it makes no sync decision.

Events (`live {event, now, entropy, …}`):

| Event | Meaning |
| --- | --- |
| `start`, `stop`, `pause`, `resume`, `wake`, `next` | Lane controls and the timer, as for the push lane's `connection` command. |
| `message {epoch, body}` | A frame arrived on the socket of this epoch: the acknowledgement or a page. The host is the producer: it hands every frame over in arrival order and decides nothing. |
| `catchUp {epoch, body}` | The response to a `request` action of this epoch. |
| `overflow {epoch}` | The host's frame buffer overflowed and frames were dropped. |
| `closed {epoch}` | The socket of this epoch closed, or a request of it failed. The host has already reported the error and refreshed credentials if it chose to. |

Actions, returned in order by every command:

| Action | Host does |
| --- | --- |
| `open {epoch, subscribe}` | Open the socket and send the subscribe frame once it is open. Frames are `message` events of this epoch; the socket's end is `closed`. |
| `request {epoch, body}` | `POST /sync/pull` with one request for every subscribed channel; the response is `catchUp`, a failure is `closed`. |
| `close {epoch, reason?}` | Close the socket of this epoch and abandon its request. A `reason` is a protocol violation to report as an error. |
| `wake {lane: "push"}` | A page applied: wake the push lane so it re-evaluates what is eligible to send. |
| `report {reports}` | What a page could not apply (read failures, skipped changes, conflicts, divergences): deliver each to the application's `onError` as an `AxtonReport`. |
| `wait {millis}` | Nothing to do until the timer fires; then report `next`. |

## 5. Building Block View

Rust owns the snapshot of the channels and the subscription generation, the epoch that invalidates every frame and response of an older session, the catch-up loop, the rule that an acknowledged session catches up before trusting the stream, gap and overflow recovery, and the wake after an applied page.

- **Epoch.** Every session has one. An I/O event names the epoch it belongs to, so whatever an abandoned socket or request still delivers is ignored. The host does not need to know why a session ended.
- **Subscription generation.** The engine counts committed subscribes and unsubscribes ([Frontend interface](../../frontend-interface.md), `subscription_generation`). A session records the value it started under; the next event after a change ends it without backoff and starts one with the new channel set. The SDKs abandon the current socket as soon as `subscribe` or `unsubscribe` is called, so no request is started for a set that is about to change, and wake the lane once the change commits.
- **Heads and catch-up.** The acknowledgement carries each channel's head. Heads equal to the durable cursors mean no catch-up at all; otherwise one `request` from the cursors, repeated while any channel continues.
- **The frame queue.** Streamed frames enter an in-memory queue (the session is the consumer). The consumer takes frames from the front through the gate: `applied` and `covered` frames leave the queue; a frame with a gap on any channel **stays** at the front and one `request` runs from the durable cursors if none is in flight. When the response is applied, the queue is checked again from the front: frames that now connect are applied, frames the pull covered are discarded, frames that still do not connect stay. A frame leaves the queue only by being applied or covered. The queue is bounded (`QUEUED_FRAMES`, 64); overflow discards it and recovers every channel from the durable cursor after the request in flight, since the server's log is the durable queue and the cursor is the pointer into it.
- **Recovery.** A host `overflow` is handled like a queue overflow, because which frames were lost is unknown; the request in flight keeps its progress, so sustained traffic cannot starve the catch-up that advances the durable cursor.
- **Failures.** A socket close, a request failure, an unconfirmed acknowledgement, a page before the acknowledgement or a malformed frame ends the session; the lane retries with the [scheduling](scheduling.md) backoff. A subscription change, `pause` and `stop` end it without backoff.

Code: [client/live.rs](../../../../../../crates/client/src/live.rs); dispositions in [client/transport.rs](../../../../../../crates/client/src/transport.rs) (`receive_downlink`); the host loops in [client-js/connection.mts](../../../../../../packages/client-js/connection.mts) (`startLiveLane`) and [dart/connection.dart](../../../../../../packages/dart/lib/src/connection.dart) (`LiveLane`).

## 6. Runtime View

1. `start` or a `wake` on an idle lane snapshots the subscribed channels and the generation. With no channels the session ends successfully and the lane stays idle until a subscribe wakes it. Otherwise `open` carries the subscribe frame ([Protocol / Subscriptions](../../../protocol/subscriptions.md)) with the read contracts from the client's schema; catch-up requests carry the same declaration.
2. The first frame must be an acknowledgement for exactly the requested channels. If every head equals the durable cursor, the session is streaming at once; otherwise a `request` starts the catch-up and repeats while any channel continues. Frames that arrive meanwhile wait in the queue.
3. Streamed frames drain through the gate as described above; an applied frame wakes the push lane; reports from every applied page and response are returned as `report`.
4. The session ends with `close`: on a failure the answer also carries `wait`, and `next` after it opens a new socket that subscribes again; on a subscription change the new session opens in the same answer; on `pause` or `stop` nothing follows.

## 9. Architecture Decisions

### The live session is a Rust state machine; hosts execute its actions ([#58](https://github.com/zanminwang/axton/issues/58))

**Decision.** The session logic that existed twice (`connect` in the TypeScript and Dart clients) is the Rust `LiveSession`, driven like `ConnectionDriver` and `SyncCycle`: the host feeds events, Rust answers with actions, and the host performs sockets, HTTP and timers. The server's per-scope drain policy is `Subscriptions` in `axton_server::live` ([Server / Connection / Controller](../../../server/connection/controller.md)). The connection model stays as chosen: HTTP writes, HTTP catch-up after the WebSocket acknowledgement, then live pages; no polling.

**Implemented contract.** Sections 3 and 5 describe it. It refines the contract this decision first sketched in four places, each chosen to keep the host without decisions:

- No `opened` or `subscriptionsChanged` event. `open` carries the frame, and Rust observes subscription changes itself through the engine's generation, so a host cannot forget to report one.
- No `acknowledged` event and no `apply` action. Every frame is a `message`; Rust tells an acknowledgement from a page ([Protocol / Subscriptions](../../../protocol/subscriptions.md)), applies pages itself, and answers `wake` when the push lane should run. The push lane's own `connection` command is unchanged.
- The live lane's scheduling lives inside `LiveSession` (`start`, `pause`, `resume`, `wake`, `next`, `wait`), so a host drives one state machine per lane.
- Streamed frames are queued by Rust, bounded, rather than by the host; the host's buffer only bounds delivery. The acknowledgement's heads let a client that is already current skip catch-up entirely ([#95](https://github.com/zanminwang/axton/issues/95)).

**Consequences.** Both SDKs shrink to transport code with no sync decisions; the transition tests run once in Rust; the target architecture's code map is true for the live session. Dart's larger frame buffer and TypeScript's smaller one are host parameters ([Transport](../transport.md)); the session's own bound is the same in both.

**Validation.** Transition tests in [sqlite/tests/live.rs](../../../../../../crates/sqlite/tests/live.rs) and the binding tests in [bindings/common/tests/session.rs](../../../../../../bindings/common/tests/session.rs); the SDK integration tests for cancellation, overlap, reconnect and after-commit delivery pass unchanged, since they assert observable behavior; `bash integration/e2e/run.sh` with both clients.

## 10. Quality Requirements

- **The session subscribes, pulls only when behind, and then streams; heads equal to the cursors mean no catch-up; one pull covers every channel and continues while any is full.** Evidence: [sqlite/tests/live.rs](../../../../../../crates/sqlite/tests/live.rs) `a_session_subscribes_pulls_only_when_behind_and_then_streams` (which also holds a gap frame, pulls, and applies it after), `heads_equal_to_the_cursors_mean_no_catch_up_at_all`, `one_pull_covers_every_channel_and_continues_while_any_channel_is_full`.
- **The frame queue is bounded and overflows into recovery; overflow discards the queue and recovers every channel after the request in flight; reports reach the host as actions.** Evidence: `the_frame_queue_is_bounded_and_overflows_into_recovery`, `overflow_discards_the_queue_and_recovers_every_channel_after_the_request_in_flight`, `reports_reach_the_host_as_actions`.
- **A subscription change ends the session without backoff; a dropped socket reconnects with backoff; protocol violations close with a reason; pause, resume and stop; a page from an earlier subscription is stale.** Evidence: `a_subscription_change_ends_the_session_and_the_next_one_uses_the_new_set`, `a_dropped_socket_reconnects_with_backoff_and_resubscribes`, `protocol_violations_close_with_a_reason_and_retry`, `pause_ends_the_session_without_backoff_resume_reopens_and_stop_is_final`, `a_page_from_a_previous_subscription_is_stale_not_a_gap_through_the_session`; [session.rs](../../../../../../bindings/common/tests/session.rs) `incoming_pages_share_cursor_policy_and_do_not_overwrite_push_cycle`, `incoming_overlap_is_identical_with_or_without_http_request_metadata`.
- **Both SDKs: listeners before catch-up, overlaps without HTTP, gaps recovered, subscription changes discard old frames, and reports reach `onError` as `AxtonReport`.** Evidence: [live.test.mjs](../../../../../../integration/bindings/client-js/live.test.mjs) `unified connection acknowledges listeners then catches up through HTTP before live delivery`, `one incoming page path covers duplicates, applies overlap directly and recovers genuine gaps`, `client replaces subscriptions from saved cursors and guards queued obsolete pages`, `what a page cannot apply reaches onError as an AxtonReport: read failures, skipped changes and divergence`, `a queued edit whose replay fails over new authority is reported as diverged and still sent`; [live_test.dart](../../../../../../packages/dart/test/live_test.dart) (the same scenarios).
- **A commit observed during catch-up is not missed, and reconnect resumes from the persisted cursor.** Evidence: [round-trip.test.mjs](../../../../../../integration/e2e/round-trip.test.mjs).

Executed 2026-09-16: `cargo test -p axton-sqlite --test live --locked`, `cargo test -p axton-binding --locked`, `node --test integration/bindings/client-js/*.test.mjs`, `dart test` in `packages/dart`; the full gate for e2e.

## 11. Risks and Technical Debt

**Accepted limitation.** A gap frame waits at the front of the queue until the pull that fills it returns; frames behind it wait too. Beyond 64 queued frames the session recovers every channel instead. Correctness does not depend on the bound; only the number of catch-up requests does.
