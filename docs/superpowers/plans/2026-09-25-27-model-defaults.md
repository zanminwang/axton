# Model creation defaults Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship typed literal/enum and once-evaluated UUID/time creation defaults across local CRUD and both Mutation delivery paths.

**Architecture:** Compiler metadata defines defaults; a focused Rust client preparation pass materializes omitted create values before strict normalization and persistence. Core/server validation stays deterministic. Generated create input types preserve omission; full records and handler inputs remain complete.

**Tech Stack:** Rust core/compiler/client, SQLite, TypeScript/React Native, Dart, existing PostgreSQL integration hosts.

## Global constraints

Follow the [spec](../specs/2026-09-25-27-model-defaults-design.md), [#27](https://github.com/zanminwang/axton/issues/27), repository AGENTS and [testing guide](../../engineering/testing/running.md). User approved constants, enum, uuid() and now() in one release. This assignment writes and reviews documents only. The user will assign a separate implementation agent; Sol is the preferred implementation model if delegating. Preserve Query/Mutation #157 and store behavior. Keep existing create return contract. No generator execution during Loader reads, replay, retry, schema backfill or old-result projection. Do not modify #151 Bootstrap semantics or implement #158.

## Task 1: Parse and validate default metadata

**Files:** `crates/compiler/src/parse.rs`, `validate.rs`, `generate.rs`; `crates/core/src/schema.rs`, public exports; tests `crates/compiler/tests/parse.rs`, `compiler.rs`, `crates/core/tests/contracts.rs`.

**Interfaces:** Add optional tagged `FieldDescriptor.create_default`, serialized `createDefault`, with variants Literal { value: Value }, Uuid, Now. Keep the existing internal `default: Option<Value>` untouched for its older migration/read-projection uses; source @default emits createDefault only. Update Rust struct literals as needed without unrelated refactors.

- [ ] Write failing compiler tests for the full Todo example in the spec and UUID-typed id, Float, nullable defaults, quoted DateTime/UUID literals. Assert normalized createDefault literal values and generator tags, with no evaluation during compilation.
- [ ] Test duplicate/default-on-relation/default-on-list/default-on-operation-field, wrong types, invalid enum members, unknown generator, nonzero generator arity, unsafe Int and invalid date/UUID literal diagnostics. Run compiler/core tests and observe failures.
- [ ] Implement a dedicated positional default-expression parser that preserves literal versus enum identifier versus zero-argument function; validate against field type before descriptor emission. Do not allow arbitrary function evaluation.
- [ ] Validate equivalent hand-written descriptors in core. Keep absence compatible; validate generator type and exclusivity. Extend fixtures/constructors that instantiate FieldDescriptor directly.
- [ ] Run `cargo test -p axton-core -p axton-compiler --locked`; commit the metadata contract.

## Task 2: Materialize defaults once in Rust client preparation

**Files:** create `crates/client/src/defaults.rs`; modify `crates/client/src/lib.rs`, `actions.rs`, `mutate.rs`; tests `crates/sqlite/tests/actions.rs`, `client.rs` or a focused new `defaults.rs`.

**Interfaces:** A shared client helper accepting schema, Model name and a mutable create object fills only missing keys; a Mutation-argument preparation helper traverses Model.create operands using retained input field membership and current default policy. Use existing chrono/uuid dependencies.

- [ ] Write tests creating local records and durable/direct Mutations with omitted defaulted id/state. Assert UUID v4, UTC-millisecond timestamps, fixed values and distinct fresh creates. Explicit values/nulls, absent non-default fields, updates and transaction rollback get separate assertions.
- [ ] Implement the preparation order:

```text
fresh create input -> fill absent declared defaults -> strict key/state normalization
fresh Mutation args -> fill create operands -> normalize_action_args -> validate bindings
                     -> derive operations / persist intent OR form direct request
```

  For local split identity/values operations, materialize without accepting misplaced/unknown keys. Optional absent operands remain absent; list items each get their own defaults. Do not evaluate generators in core::normalize_state or server decoding.
- [ ] Persist expanded arguments and operations atomically on durable acceptance. Verify frozen bytes, call arguments and optimistic rows share exactly the same id/time; restart/retry/rebuild never regenerates them. Existing low-level enqueue create support must also store concrete values before replay.
- [ ] Add malformed server/direct request tests proving required defaults are not synthesized server-side, and Loader tests proving missing required fields still fail.
- [ ] Run `cargo test -p axton-client -p axton-core -p axton-server -p axton-sqlite --locked`; commit the execution boundary.

## Task 3: History and storage preserve concrete values

**Files:** `crates/compiler/src/history.rs`, `crates/core/src/schema.rs`, `crates/client/src/ddl.rs` only as necessary; tests `crates/compiler/tests/history.rs`, `cli.rs`, `crates/sqlite/tests/ddl.rs`, `rebuild.rs`, `crates/core/tests/compatibility.rs`.

**Interfaces:** Default policy is independent of backend field shape. Retain old versions; default-only edits to current-version metadata do not change structural compatibility.

- [ ] Add tests changing/adding/removing constants and generators on an existing field at the same Model/Mutation version. Project createDefault out of capture_model read snapshots, resultModels and retained operation-input snapshots; preserve older internal default metadata. Compare structural contracts without creation policy while preserving name/type/nullability, identity, enum and required-field fences. Current client Model descriptors retain creation policy for the prefill pass.
- [ ] Verify open/reopen with a changed default leaves existing rows, queued args and frozen bytes unchanged; new creates use the new policy.
- [ ] Verify older internal literal backfill paths still work, but source createDefault metadata (including literals) never supplies migration values. New required fields follow the existing version/rebuild rules; nullable new fields follow existing null-fill behavior. No creation metadata may be interpreted by SQL literal rendering, migration carry-over or current-authority projection.
- [ ] Keep required-field read/version changes requiring the existing version bump, even with a default. Add regression assertions alongside default-only compatibility.
- [ ] Run the compiler/core/sqlite suites; commit compatibility changes.

## Task 4: Typed create inputs and safe generated sources

**Files:** `crates/compiler/src/emit.rs`, name validation as necessary; tests `crates/compiler/tests/compiler.rs`; fixtures under `integration/action-contract`, `integration/action-runtime-ts`, `integration/action-runtime-dart`, `integration/generated-api` and their negative checks.

**Interfaces:** TS ModelCreate is an omission-preserving client input with defaulted fields optional; Model and backend handler arguments retain complete field types. Dart uses a dedicated create input and presence representation for nullable defaults, keeping explicit full Model inputs usable where practical. Model-only schemas must work too.

- [ ] Add positive and negative TS/Dart contracts demonstrating omitted defaults accepted only for create, while omitted required fields, incomplete backend values and invalid default values fail.
- [ ] Emit create encoders that omit missing fields rather than encode null/undefined through date or identity encoders. SDK create encoders omit absent identity keys; Rust fills and normalizes those keys before record-key construction, persistence or direct dispatch. Supplied identities retain existing normalization; do not reuse an encoder that unconditionally emits all identity keys. Never generate UUID/time in SDK code.
- [ ] Ensure current and historical handler inputs require concrete values despite optional client inputs. Cover optional/list create operands and local transaction create.
- [ ] Replace unsafe Dart raw triple-quoted schema embedding with safe string encoding. Generate/compile/run default strings containing single/double/triple quotes, dollar signs, backslashes and newlines; assert round-trip values.
- [ ] Build prerequisites per running guide; run `bash integration/generated-api/verify.sh`, `bash integration/action-runtime-ts/verify.sh` and corresponding Dart analyzers/tests/negative fixtures. Commit generator and fixture updates.

## Task 5: Runtime acceptance, documentation and final integration

**Files:** `integration/action-e2e/source/app.model`, `backend-fixture.ts`, `action.test.mts` and regenerated history/output as needed; JS/RN bindings tests, `packages/dart/test/actions_test.dart`; owning schema/models, types, compiler/generate, storage/reconciliation docs; website schema reference and examples.

- [ ] Exercise local create and both Mutation routes through real SDK/native/database boundaries. Backend assertions compare id/time/default fields with local optimistic state and retry/reopen observations. Returned records use canonical Loaders and cannot silently fill missing required output.
- [ ] Update docs with creation-only rules, explicit null handling, client-clock semantics, create-only versus explicit migration behavior, typed create usage, unchanged local create return and backend field completeness. Replace obsolete grammar-default debt claims and validate documentation examples/links.
- [ ] Integrate latest main. If #151 has merged, preserve its compiler/storage/fixture changes and run its Bootstrap regressions alongside the default tests; otherwise report that combined verification remains for the second merge.
- [ ] Run `bash integration/action-e2e/run.sh`, relevant binding/Dart tests and `bash scripts/test.sh`. Inspect generated artifacts, `git diff --check`, and all test failures; record actual evidence and limitations.
- [ ] The assigned implementer reviews spec coverage and final diff, fixes findings, opens PR with `Closes #27`, attaches PR to its task, checks CI and follows the implementation handoff merge authorization. Update issue state and report merge commit/evidence. This preparation task does not execute these steps.

## Preparation evidence

Isolated branch `codex/27-model-defaults` starts from `ceced50`. Baseline `cargo test -p axton-core -p axton-compiler -p axton-sqlite --locked` completed successfully before implementation (log `/tmp/axton-27-baseline.log`). 329 tests passed with 0 failures in 26 suite summaries. This verifies existing behavior, not the new feature. Checkboxes track future implementation.
