# Pull

Engine behavior: [Client Pull](../client/engine/pull.md), [Server Pull](../server/engine/pull.md).

## 3. Context and Scope

- Request, `POST /sync/pull`: `{models, cursors}`. `cursors` maps every channel the client follows to its durable cursor (`0` on first contact): one request covers all of them. `models` declares the read contracts the client expects, `{"Entry": 2, "Comment": 1}`: every model of its schema with the version its generated types read ([#91](https://github.com/zanminwang/axton/issues/91)); a client still on `Entry` v1 sends `{"Entry": 1}` and never sees fields v2 added. The SDK fills both; application code never writes them. There is no client id: the owner comes from authentication.
- Response, a page: `{cursors: {channel: {from, to, head}}, changes: [...]}`. The same page shape is streamed over the WebSocket, naming only the channels that moved ([Subscriptions](subscriptions.md)).
- A change is `{model, identity, stamp, state}`: the same authority record a push receipt carries ([Push](push.md)). `state` is the whole record or `null` for a deletion. A record the server could not read is `{model, identity, stamp, state: null, error}` where `error` is a code: `loader.failed` for a thrown loader, or the loader's own refusal code.
- **Mode.** A `mode` key selects what the route serves, before either shape decodes: absent is the ordinary delta pull described above, the string `bootstrap` is one bounded page of a Scope's historical interval (below), and any other present value (`null`, a number, another string) is `400 request.invalid`. The dispatch lives in the shared engine entry point, so every binding, the HTTP route and a direct backend call share it.
- Bootstrap request, the same `POST /sync/pull`: `{mode: "bootstrap", channel, models, after, until}`. One channel, not a cursor map; `models` is the same read-contract declaration; `after` is the client's committed historical progress B and `until` the subscription's origin S, fixed for the whole walk ([#151](https://github.com/zanminwang/axton/issues/151)). Counters are safe and `0 ≤ after ≤ until`.
- Bootstrap page: `{mode: "bootstrap", channel, from, to, until, head, records}`. `records` holds the same authority records a delta page carries, at most 50. The page echoes `channel`, `from` (the requested `after`) and `until`; `head` is the channel head its transaction observed, which the client stores as its completion barrier on the final page. `to == until` is completion, so no done flag travels; `from ≤ to ≤ until ≤ head` always holds. The client rejects a mismatched echo, backwards progress, a nonterminal page that made no progress, and more than 50 records (`BootstrapPage::answers`).
- Errors: `400 request.invalid` when a cursor is ahead of its channel head, `models` or `cursors` is missing or malformed, a bootstrap request's channel is blank or its `after` is past its `until`, its `until` is above the channel head, the `mode` is present and unsupported, or the body is malformed; `409 model_version_unsupported` with `{model, version}` when a declared model is unknown or its version is not retained; `500 server` for infrastructure failures. A loader failure is not a request error.

```json
{"models":{"Entry":2,"Comment":1},"cursors":{"book:demo":42,"inbox:alice":7}}
{"mode":"bootstrap","channel":"project:123","models":{"Entry":2},"after":40,"until":100}
```
```json
{"cursors":{"book:demo":{"from":42,"to":47,"head":47},"inbox:alice":{"from":7,"to":9,"head":9}},
 "changes":[
  {"model":"Entry","identity":{"id":"e"},"stamp":12,"state":{"text":"Hello","note":null}},
  {"model":"Entry","identity":{"id":"f"},"stamp":3,"state":null},
  {"model":"Entry","identity":{"id":"g"},"stamp":5,"state":null,"error":"loader.failed"}
 ]}
```
```json
{"mode":"bootstrap","channel":"project:123","from":40,"to":100,"until":100,"head":140,
 "records":[{"model":"Entry","identity":{"id":"e"},"stamp":12,"state":{"text":"Hello","note":null}}]}
```

## 5. Building Block View

- **Page rules.** Every channel range has `from ≤ to ≤ head`; at least one channel is named; a record appears once (no two changes share `(model, identity)`); every change carries `state` and a positive `stamp`; `error`, when present, is a valid code and then `state` is `null`. A page holds at most `limits::PULL_CHANGES` (50) changes per named channel.
- **Changes carry no channel and no cursor.** The server collects the invalidations of every channel into one set keyed by record, so a record published to two followed channels is delivered once, at its current stamp. Channels say which clients receive a change; they never appear on a record.
- **Two counters, two jobs.** A channel's `from`/`to` orders delivery within that channel (guarantee A2); `stamp` orders content per record across every delivery path (guarantee D2).
- **Per-channel continuation.** Each channel scans at most 50 invalidations. A channel whose `to` is below its `head` continues (`CursorRange::continues`) and the client pulls again from `to`; the other channels are not held back.
- **A change is a whole record**, shaped by the declared version of its model; the same record and stamp reach a v1 client in the v1 shape and a v2 client in the v2 shape.
- **Bootstrap invariants.** One channel per request, and the interval is half-open below and closed above: the page covers `(from, to]` and a record published exactly at `until` is historical. The upper bound never moves, so the walk terminates under sustained publication - a record republished above `until` leaves the interval and the subscription delivers it instead ([Subscriptions](subscriptions.md)). `head` is read in the page's own transaction and is therefore the only counter that may move between pages; it is not a bound the client chases. A nonterminal page must advance `to` beyond `from`.

Code: [core/protocol.rs](../../../../crates/core/src/protocol.rs) (`PullRequest`, `PullPage`, `CursorRange`, `AuthorityRecord`, `BootstrapRequest`, `BootstrapPage`, `pull_mode`, `limits`).

## 10. Quality Requirements

- The canonical bootstrap request and page and every case in [bootstrap-request.json](../../../../fixtures/protocol/bootstrap-request.json) and [bootstrap-page.json](../../../../fixtures/protocol/bootstrap-page.json) decode as declared: an unknown, null, numeric or missing mode, missing and unsafe counters, `after > until`, a blank channel, `to > until`, `until > head`, a page over the record limit, and either mode decoded as the other. A page answers only the request it continues: another channel, origin or `from`, and a nonterminal page repeating its `from`, are refused. Evidence: [core/tests/contracts.rs](../../../../crates/core/tests/contracts.rs) `bootstrap_request_fixture_cases_decode_as_declared`, `bootstrap_page_fixture_cases_decode_as_declared`, `a_bootstrap_page_answers_only_the_request_it_continues`.
- One entry point dispatches both modes, the ordinary page is unchanged, and any other mode is refused over HTTP and through a direct backend call. Evidence: [server/tests/bootstrap.rs](../../../../crates/server/tests/bootstrap.rs) `a_bootstrap_request_is_refused_like_an_ordinary_pull_when_it_is_malformed`, `the_ordinary_pull_page_is_unchanged`; [runtime.test.mjs](../../../../integration/persistence/server/runtime.test.mjs) `the pull route dispatches by mode over HTTP and refuses any other mode`.
- The canonical page and every case in [pull-page.json](../../../../fixtures/protocol/pull-page.json) decode as declared: a record shared by two channels, a deletion, an error change; refusals for a duplicate record, `to < from`, no channels, a change with both state and error, an error that is not a code, a legacy single-channel page, and a page over the per-channel limit. Evidence: [core/tests/contracts.rs](../../../../crates/core/tests/contracts.rs) `pull_page_fixture_cases_decode_as_declared`, `a_page_names_its_channels_and_keeps_unknown_fields_out_of_the_records`, `shared_limits_are_defined_once_and_apply_per_channel`, `shared_wire_fixtures_preserve_counter_boundaries`.
- A pull carries no client id and declares the read contracts; a missing or bad declaration is refused. Evidence: `push_requests_refuse_a_blank_client_id_and_pulls_carry_none`, `pull_and_subscribe_declare_the_read_contracts_and_refuse_a_missing_or_bad_declaration`.
- One pull covers every channel, a shared record arrives once, and a full channel continues on its own. Evidence: [server/tests/stamp.rs](../../../../crates/server/tests/stamp.rs) `one_pull_covers_every_channel_and_delivers_a_shared_record_once`, `a_full_channel_continues_independently_of_the_others`, `a_cursor_ahead_of_its_channel_head_is_refused`; [runtime.test.mjs](../../../../integration/persistence/server/runtime.test.mjs) `a pull covers every channel in one request and delivers a record shared by two channels once`.
- A record that cannot be read is an `error` change and the page is served. Evidence: `a_loader_refusal_isolates_one_record_after_a_per_identity_retry`, `a_loader_failure_is_an_error_change_and_a_single_record_needs_no_retry`; `a loader that throws for one id fails only that record and reaches onError`, `a loader refusal for one id is an error change carrying the refusal code`.

Executed 2026-09-25: `cargo test -p axton-core -p axton-server --locked`, `bash integration/persistence/server/run.sh`.

## 11. Risks and Technical Debt

- **Accepted limitation (planned change).** The per-channel limit is the fixed 50 ([#11](https://github.com/zanminwang/axton/issues/11)), and a bootstrap page carries at most the same 50 records. Snapshot transport instead of a cursor walk is [#14](https://github.com/zanminwang/axton/issues/14).
- **Accepted limitation.** An `error` change is not retried by the protocol; the record is corrected the next time it is published or returned by an Action output with storage enabled.
