# Incremental Model watch delivery

> Execution: continue inline in the existing isolated worktree. The user's handoff approves the design and scope; no new design approval is needed.

**Goal:** Keep Rust authoritative for Model watch membership and ordering, send only changed rows after the first result, and avoid decoding unchanged typed Models while preserving mutable snapshot semantics.

**Architecture:** A watch opts into `spec.delivery = "splice-v1"`. Rust publishes a versioned full result, then sequential ordered splices with `baseVersion` and `version`. Bulk refresh and recovery publish full results. Older callers receive legacy snapshots; older natives ignore the opt-in and remain supported. SDK-owned caches decode incoming rows only, then copy values at the application boundary. Dart reuses immutable scalar-only Models, copies nested Lists, and JS copies objects, Lists and Dates. `watchSql` retains full dependency-table snapshots.

**Tech stack:** Rust runtime/protocol/compiler, TypeScript shared Node/RN runtime, Dart native SDK, real SQLite and release Dart AOT/PostgreSQL performance fixtures.

## Constraints

- Scope is single-Model equality filters. No arbitrary SQL incremental evaluation.
- No mutation of already delivered results; no application mutation may corrupt internal cache.
- Retain cancellation, close, committed visibility, publication order and read-error recovery.
- Preserve legacy SDK/native combinations and public watch signatures.
- Use instrumentation 3 before and after, with the existing clock-correlated trace exporter. Never sum overlapping durations.
- Work only in `/tmp/axton-model-watch-incremental` on `codex/model-watch-incremental`.

## Tasks

- [x] Capture pre-delivery instrumentation-3 measurements at HEAD 9895d13c: `bash integration/performance/run.sh --samples 3 --sizes 100,1000,10000 --writes 100 --output /tmp/axton-watch-before-v3.json`. Retain all failures and raw traces.
- [x] Add Rust real-SQLite regressions in `crates/sqlite/tests/model_watch_incremental/mod.rs`: initial versioned result; changed-row payload bound with 1,000 retained rows; sequential insert/replace/delete replay equals authoritative query; no-op suppression; explicit full-snapshot recovery; bulk reset and close; legacy behavior. Run `cargo test -p axton-sqlite --test protocol05_runtime model_watch_incremental` and observe failure before changes.
- [x] Add optional delivery negotiation to `crates/protocols/src/client_bridge/mod.rs`. Extend `Watch` in `crates/client/src/runtime/observers.rs` with version/pending splices. `update_keys` records its exact ordered edits while updating the cache. `replace` selects a full snapshot. Publication moves patches without cloning all cached rows. Add `watchSnapshot {observerId}` as an in-memory authoritative recovery, routed by `runtime/tasks.rs` and refused in ordinary command execution. Run focused Rust tests, then client/sqlite/protocol tests and Clippy.
- [x] Add `WatchRows<T>` receivers under the JS and Dart API directories. Tests replay full and splice messages, count decodes (initial N plus changed rows), mutate returned nested Lists/Maps/Dates, retain old results, reject malformed/version-mismatched patches atomically, and accept a newer full recovery. Legacy full snapshots remain supported. Build complete decoded splice edits before changing cache, validate sequential bounds and versions, and copy on public delivery.
- [x] Wire generic Model `watch` and internal `watchDecoded` through the receivers in `client-js/api/runtime.mts` and `dart/lib/src/api/client.dart`. Give every Dart listen its own receiver. On a broken patch report the error and request `watchSnapshot`; ignore intervening patches until a full recovery. Keep listener exceptions separate from decoding failures. Test lifecycle with real native SQLite plus existing SQL-watch suites and fake-carrier mixing tests.
- [x] Update `crates/compiler/src/emit.rs`: generated watches use `watchDecoded`; emit schema-specific copy functions that duplicate JS objects/Date/List values and Dart List-bearing records without rerunning decoding. Keep scalar-only Dart Models reusable. Preserve older generated code via generic `watch`. Add generated native TypeScript/Dart tests that mutate results and verify later unchanged rows, dates and list fields remain canonical. Regenerate checked-in fixtures through their owning runners, typecheck, analyze and run native tests.
- [x] Review protocol, SDK and generator changes against cancellation, close, recovery, mixing and cache ownership. Update owning architecture docs and performance guide. Run affected release/native host E2E and retain actual command results.
- [ ] Rebuild release native/compiler and Dart AOT; run the full 45-sample matrix (`--samples 5 --sizes 100,1000,10000 --writes 100`). Retain raw data and common-axis traces. Compare compatible conditions against the pre-delivery baseline; inspect a median single-write sample and report actual B visibility plus remaining overlapping stages. Archive compressed results and an audit script in a dated performance baseline directory.

## Verification before the optimized performance run

The 27-sample instrumentation-3 pre-delivery matrix passed. Client/SQLite/protocol/compiler tests and strict Clippy passed. Generated native Dart tests (11) and protocol-5 host E2E (21) passed; generated TypeScript checks and native Node scripts passed. SDK receiver, lifecycle and SQL-watch regressions passed. Independent review found no production defects; executing a generated DateTime-list mutation-isolation scenario remains optional additional coverage (scalar Date and mutable string Lists are exercised through generated native APIs). Full host gate and installed-package release gates are outside this focused change verification.
