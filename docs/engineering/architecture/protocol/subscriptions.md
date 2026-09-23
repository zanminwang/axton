# Subscriptions

Engine behavior: [Client / Connection / Controller](../client/connection/controller/README.md), [Server / Connection / Controller](../server/connection/controller.md).

## 3. Context and Scope

- Endpoint: WebSocket upgrade on `/sync/live` with `Authorization: Bearer <token>`; refused with a raw `401`, `500` or `503` before the upgrade.
- Client frame, exactly one: `{"type":"subscribe","channels":[…],"models":{…}}`. `models` is the same read-contract declaration as on a [pull](pull.md) and required. It carries no cursors and no client id; any other key is refused.
- Server acknowledgement: `{"type":"subscribed","cursors":{channel: head}}`: every requested channel with its current head.
- Server frames: [Pull](pull.md) pages without a `type` key, naming only the channels that moved; each channel's `from` is where the previous frame (or the acknowledgement's head) left it.
- Close codes: `1002` protocol violation (a second client frame, a malformed subscribe, or a declaration the server refuses, with reason `model_version_unsupported`), `1011` server failure, `1001` server shutting down.

```json
{"type":"subscribe","models":{"Entry":2,"Comment":1},"channels":["book:demo","inbox:alice"]}
{"type":"subscribed","cursors":{"book:demo":47,"inbox:alice":9}}
{"cursors":{"book:demo":{"from":47,"to":48,"head":48}},
 "changes":[{"model":"Entry","identity":{"id":"e"},"stamp":13,"state":{"text":"Hello!","note":null}}]}
```

## 5. Building Block View

- Channels must be non-empty strings and at least one is required. `SubscribeRequest` normalizes them (deduplicated, UTF-16 order); `SubscriptionAck::confirms` checks that the acknowledgement names exactly that set. `LiveMessage` tells an acknowledgement (it has a `type`) from a page (it has none).
- The declaration is checked at the handshake and kept for the session; every frame it streams is loaded at those versions. The acknowledgement is produced inside the negotiating transaction, which also reads each channel's head; streaming starts there.

Code: [core/protocol.rs](../../../../crates/core/src/protocol.rs) (`SubscribeRequest`, `SubscriptionAck`, `LiveMessage`); the server side in [server/live.rs](../../../../crates/server/src/live.rs); the client side in [client/live.rs](../../../../crates/client/src/live.rs).

## 6. Runtime View

HTTP catches up; the WebSocket carries only what is new. On the acknowledgement the client compares each head with its durable cursor: all equal, it consumes the stream at once; any behind, it issues one [pull](pull.md) from its cursors, repeated while a channel continues, then consumes the stream. A frame whose `from` does not meet the local cursor of a channel is a gap: the frame is held and a pull fills the gap ([Live session](../client/connection/controller/live-session.md)). Changing the channel set means closing the socket and negotiating again; there is no resubscribe frame.

## 10. Quality Requirements

- Subscribe, acknowledgement and frame decode, normalize and refuse as the shared fixture says. Evidence: [core/tests/contracts.rs](../../../../crates/core/tests/contracts.rs) `live_frames_decode_as_acknowledgement_or_page_and_channels_normalize` over [fixtures/protocol/live-messages.json](../../../../fixtures/protocol/live-messages.json).
- Only one subscribe frame is accepted and channels are normalized; the acknowledgement carries heads; commits on several channels share one pull and a frame names only what moved. Evidence: [server/tests/runtime.rs](../../../../crates/server/tests/runtime.rs) `live_subscribe_requires_one_subscribe_frame_and_normalizes_channels`, `live_page_progression_checks_every_channel_it_asked_for`; [server/tests/live.rs](../../../../crates/server/tests/live.rs) `open_registers_every_scope_before_the_acknowledgement_then_pulls_all_once_from_their_heads`, `a_channel_below_its_head_continues_and_one_at_its_head_ends_the_drain`, `commits_on_several_scopes_share_one_pull_and_the_frame_names_only_what_moved`.
- A client whose cursors equal the heads does not catch up; one that is behind pulls once and then streams. Evidence: [sqlite/tests/live.rs](../../../../crates/sqlite/tests/live.rs) `heads_equal_to_the_cursors_mean_no_catch_up_at_all`, `a_session_subscribes_pulls_only_when_behind_and_then_streams`; both SDKs' live suites ([live.test.mjs](../../../../integration/bindings/client-js/live.test.mjs), [dart/test/live_test.dart](../../../../packages/dart/test/live_test.dart)).

Executed 2026-09-16: `cargo test -p axton-core -p axton-server -p axton-sqlite --locked`, the JS and Dart live suites.

## 11. Risks and Technical Debt

- **Resolved ([#63](https://github.com/zanminwang/axton/issues/63)).** The acknowledgement's vestigial `rejections` array is gone with the new frame.
