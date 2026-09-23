# Pull

## 1. Introduction and Goals

Server pull answers "what changed in these channels after these positions" with whole records, loaded by the application under its own visibility rules, in one page the client applies as a unit. A record that cannot be read fails alone.

## 3. Context and Scope

Input: the owner, a [pull request](../../protocol/pull.md) `{models, cursors}` (the live stream passes the same cursor map) and a host. Output: one page, or an error that aborts the request (a cursor ahead of its head, malformed invalidation rows, unregistered model, non-canonical identity, missing stamp metadata, an unusable host answer, or a thrown host error). Nothing a loader answers is a request error: a refusal, a thrown loader error, a row the contract rejects or a misaligned answer becomes that record's `error` change. Both the HTTP route and the live drain call it ([Server / Connection / Controller](../connection/controller.md)).

## 5. Building Block View

Pull reads two things: the **invalidation table**, one row per `(channel, model, record)` at the record's latest position in that channel, joined with the record's *current* stamp (`scan`; a row whose record has no stamp metadata is a storage error), and the **loaders**, which supply current content. It writes nothing. The stamp comes from the record, not the invalidation row, because a change advances a stamp whether or not it is published ([Publish](publish.md)); the cursor is delivery progress only.

Code: `process_pull` in [server/lib.rs](../../../../../crates/server/src/lib.rs); the live wrapper `pull` in [server/live.rs](../../../../../crates/server/src/live.rs).

## 6. Runtime View

1. Check the declaration (every declared model known, every version retained, else `model_version_unsupported`).
2. For each requested channel, in canonical order: read its head (a cursor beyond it is `request.invalid`); scan up to `limits::PULL_CHANGES` (50) invalidations after the cursor, checking order, head, model and identity key. The channel's `to` is the last scanned cursor when the scan was full, otherwise the head; `head` is reported alongside so the client knows whether the channel continues.
3. Merge every channel's rows into one set keyed by record. A record published to two requested channels appears once, at its current stamp.
4. Group the records by model and call that version's loader once with all identities (`load {model, version, identities, owner}`; no channel). Rows are normalized against the retained contract of the declared version; `null` is a deletion. A model the client did not declare is refused with `model_version_unsupported`.
5. **A record fails alone.** When the batched call answers a refusal or a failure, or answers a different number of rows than identities, the engine calls the loader again once per identity. The TypeScript host answers a failure for a thrown loader error and for an answer JSON cannot carry (not an array, an `undefined` entry, a nonfinite number, an out-of-range bigint). Each identity whose own call is refused or fails becomes `{…, state: null, error}` with the refusal code or `loader.failed`; a single call that answers other than one row, and any row the retained contract rejects, becomes `loader.invalid`; the others get their rows. A single-identity call needs no retry. A thrown host error (infrastructure) still fails the request.
6. Emit the changes in canonical record order with the per-channel ranges.

**Reporting.** The host passes every non-business loader error, the batched one and each per-identity one, to the backend's `onError`, and reports each `loader.invalid` change of a page it serves; without an `onError` the default is `console.error`, so nothing is dropped silently ([Backend interface](../backend-interface.md)).

**Compaction.** Because a record has one row per channel, publishing it again moves that row to a new cursor and leaves a hole at the old one. A client that already applied the old position sees the record again later with a newer stamp; the stamp makes that harmless.

**Coherence.** Head, scan and load must observe one snapshot. That is a requirement on the application's transaction runner ([Persistence](../persistence.md)); the shipped runner uses repeatable read.

## 9. Architecture Decisions

**One pull covers every channel ([#95](https://github.com/zanminwang/axton/issues/95)).** Records are applied by stamp and never carry a channel, so there is no reason to fetch channels one request at a time. One request moves every cursor, and a record shared by channels is loaded and sent once.

**Loader failure isolation ([D7](../../../guarantees.md#d-distribution)).** A failure attributable to one read is that record's `error` change; the page and the other records proceed and every cursor still advances. There is no durable loader-failure queue: the record is corrected the next time it is published or explicitly fetched. The per-identity retry keeps the loader API unchanged (one call per model) and costs extra calls only on failure; a loader that wants to refuse exactly one row refuses when called with that row. Inside a push the same refusal is that mutation's rejection ([Push §9](push.md#9-architecture-decisions)).

**Loaders name no channel ([#55](https://github.com/zanminwang/axton/issues/55)).** The record a page delivers is the record a receipt delivers at the same stamp (guarantee D4); a channel selects which records are delivered, never alternate contents. A loader that needs to hide a record from a user returns `null` for it or refuses the read.

## 10. Quality Requirements

- **One pull covers every channel; a shared record arrives once; a full channel continues on its own; a cursor ahead of its head is refused.** Evidence: [server/tests/stamp.rs](../../../../../crates/server/tests/stamp.rs) `one_pull_covers_every_channel_and_delivers_a_shared_record_once`, `a_full_channel_continues_independently_of_the_others`, `a_cursor_ahead_of_its_channel_head_is_refused`; [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `a pull covers every channel in one request and delivers a record shared by two channels once`.
- **A refused or failed read is isolated to its record after a per-identity retry; a thrown host error still fails the pull.** Evidence: `a_loader_refusal_isolates_one_record_after_a_per_identity_retry`, `a_loader_failure_is_an_error_change_and_a_single_record_needs_no_retry`, `a_thrown_host_error_still_fails_the_pull`; `a loader that throws for one id fails only that record and reaches onError`, `a loader refusal for one id is an error change carrying the refusal code`.
- **Loader errors reach `onError`, which defaults to `console.error`.** Evidence: `onError defaults to console.error so nothing is dropped silently`.
- **Every change carries the record's current stamp; compaction delivers the latest state once; loaders receive no channel; head, scan and load stay coherent.** Evidence: `scan pairs the invalidation cursor with the current record stamp; a missing record row is a storage defect`, `compaction materializes latest state; deletion is aligned null`, `loaders receive no channel`, `repeatable-read runner keeps head, scan, and loader coherent across concurrent publication`.
- **A pull is served at the declared model version.** Evidence: [server/tests/stamp.rs](../../../../../crates/server/tests/stamp.rs) `pull_normalizes_loader_rows_with_the_retained_contract_of_the_served_version`; `a pull reaches the loader of the declared model version and normalizes rows with that contract`.

Executed 2026-09-16: `cargo test -p axton-server --locked`, `bash integration/persistence/server/run.sh`.

## 11. Risks and Technical Debt

**Problem: a model the client did not declare still fails the whole pull.** A client older than the server (the server added a model) is refused with `model_version_unsupported` for any page holding that model. Reporting it as a per-record `error` like a loader failure is the natural next step; not done here.

**Accepted limitation (planned changes).** The per-channel limit is the fixed 50 ([#11](https://github.com/zanminwang/axton/issues/11)); bootstrap is a cursor walk from zero over every model ([#14](https://github.com/zanminwang/axton/issues/14) proposes snapshots).

**Accepted limitation, worth stating.** The loader is the only visibility control: a loader that ignores `userId` exposes every record it is asked for to any authenticated user, on every channel that delivers it.
