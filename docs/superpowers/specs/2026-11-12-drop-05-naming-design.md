# Drop the 05 naming

Issue: none yet. Status: approved for implementation (user, 2026-11-12); design review applied. Slug: `drop-05-naming`.

Names stop carrying `05`, `v05`, `V05` and `protocol5`. The marker told the protocol-5 implementation from the protocol-4 one beside it. One implementation is left and its wire protocol number is 6, so the marker says nothing true. This change first deletes the code that only existed to tell the two apart, then renames. Behavior does not change, with one stated exception in §2.5. Done means the searches in §5 find the marker only in the recorded residue, and the full gate passes after every task.

## Contents

1. [Problem and evidence](#1-problem-and-evidence)
2. [Design](#2-design)
3. [Changes](#3-changes)
4. [Decisions](#4-decisions)
5. [Verification](#5-verification)
6. [Out of scope](#6-out-of-scope)
7. [Open questions](#7-open-questions)

## 1. Problem and evidence

At `main` bd46f42:

- **The number is wrong.** `PROTOCOL` is 6 (`crates/protocols/src/sync.rs:8`). The names say 5.
- **The marker is everywhere.** The proof search of §5 finds 2,901 lines in 224 files today. About 160 distinct names carry it, and about 87 tracked paths outside history: `crates/client/src/store05.rs`, `settlement05.rs`, `sync05/`, `crates/server/src/protocol_v05.rs`, `crates/sim/src/scenario05.rs`, 16 `crates/sqlite/tests/protocol05_*.rs`, `integration/v05-sdk/`, JS and Dart test files, three docs. Prose says "protocol 5" on 135 more lines.
- **One module has two names.** `axton_protocols::sync` is imported as `v05` in 14 places and used as `v05::` 837 times; `axton_client` re-exports it under that name (`crates/client/src/lib.rs:36`).
- **Dead branches still choose between protocols.** The actor starts a different loop unless the open request says `"protocol": 5`, and that loop can only fail to open (`bindings/common/src/actor.rs:300-304`, `:394-421`). `Client.context05` is an `Option` that is always `Some` and never read (`crates/client/src/store05.rs:266`, `:288`). The backend probes its native addon for entry points the addon always has (`packages/backend/server/bindings/native.mts:15-49`).
- **A required option looks optional.** `BackendOptions.protocol5` is declared optional (`packages/backend/server/api/backend.mts:204`). Without it every Stream admission answers false (`packages/backend/server/bindings/host.mts:130-137`), so no client can sync. The generated backend already requires it (`crates/compiler/src/emit.rs:1133`).

## 2. Design

### 2.1 Delete what only told protocol 4 from protocol 5

These go first, each in its own commit, each with a search or test showing nothing reached it.

| Leftover | What it is | Action |
| --- | --- | --- |
| `run`, `serve`, `admit`, `flush` in `bindings/common/src/actor.rs:440-572` | The loop for an open request without `"protocol": 5`. `open_runtime` returns "protocol5 Store is required" for it (`:421`). Both SDKs always send 5 (`packages/frontend/client-js/bindings/bridge.mts:174`, `packages/frontend/dart/lib/src/bindings/bridge.dart:556`). | Delete the loop and the branch. The open message loses its `protocol` field in the core and both SDKs. |
| "storeHooks are not supported in protocol5" (`actor.rs:400-404`) | Refuses a protocol-4 open option no SDK sends. | Delete with its two tests (`bindings/common/tests/runtime.rs:120`, `:132`). |
| `SqliteStore::open_exclusive(path)` (`crates/sqlite/src/lib.rs:242-246`) | "Protocol-4 file ownership": the lock without the Stream admission. Production calls only `open_exclusive05` (`actor.rs:409`); 19 test call sites use the other. | Delete. 16 call sites swap mechanically. Three do not: `bindings/common/tests/protocol05_runtime.rs:1144` and `:1205` test that the lease is released on a Store with a corrupt counter, and the admitting open fails on that counter, so their assertion becomes "the error is not `store_in_use`"; `crates/sqlite/tests/protocol05_binding.rs:40` expects "Stream mismatch", which now comes from the Store layer. |
| `Client.context05: Option` (`crates/client/src/frontend_interface.rs:97`) | Set at open and reset, checked with `is_none()`, never read. | Delete the field, the check and its "protocol05 Store required" error. Of the calls `request_context05()?;`, seven discard the result and are deleted (`settlement05.rs:406`, `:410`, `:677`, `:690`, `:734`, `mutation_queue.rs:364`, `sync05/reset.rs:14`). Every call that uses the result stays, among them `runtime/direct.rs:428`, `settlement05.rs:423`, `store05.rs:294`, `sync05/delta_applier.rs:57`. |
| `Subscriptions.context05: Option` (`crates/server/src/live.rs:60-62`) | Always `Some`; unwrapped at `:112`. | The field holds a value. `progress05` is real state and stays an `Option`. |
| `Subscriptions::handle` and `handle_stream` (`live.rs:208-213`) | Two wrappers that both call `handle05`. | One method, `handle`. |
| `settle_external05` over `settle_external` (`crates/server/src/protocol_v05.rs:125-131`, `crates/server/src/lib.rs:207`) | The wrapper adds the host adapter; the inner function has no other caller. | One function. |
| Optional sync entry points on `Native`: `processLive05`, `processDelivery05`, `processRead05`, `handshake05`, `serverMaterializationId05`, `settleExternal05` (`native.mts:15-49`) | Their `?` exists to probe for an addon without protocol-5 support. `@axtonjs/server` pins `@axtonjs/native` exactly (`packages/backend/server/package.json:28`). | Required members. Delete the `typeof` filter and conditional spreads (`native.mts:140-200`), the non-null assertions at their call sites, and the errors at `backend.mts:361` and `connection/live.mts:210`. |
| `fixtures/protocol/0.4.json` | No code loads it. | Delete. |

Not deleted:

- **The `"protocol4"` refusal in `Config::decode`** (`crates/server/src/lib.rs:78`). Compilers up to 0.4.x wrote that key into an app's checked-in `backend.json`; the refusal is what such an app sees. It stays, with its test (`integration/persistence/server/startup.test.mjs:141-144`).
- **Capability probing for Batches and commit groups**: `validateMutationBatch`, `processBatchMember`, `encodeBatchAcknowledgement`, `processBatchGroup`, `mutationCommitGroups`, `mutationResultGroups` (`backend.mts:299`, `:308`, `:652-658`). These check what an addon or a database adapter can do, not which protocol it speaks. Only the message at `:658` loses the marker.
- `refused_acts05`, `status_snapshot05` and `materialization_id05` are live and are only renamed.

Once the options are flat (§2.5) the materialization ID is always derived, so every partial `native` fake needs `serverMaterializationId`: `integration/persistence/server/database-startup.test.mjs:10`, `host-contract.test.mjs:86` and `packages/backend/server/bindings/host.test.mjs:86` gain it; `integration/generated-api/backend-complete.ts:29`, `integration/persistence/server/live-heartbeat.test.mjs:39`, `mutation-defaults.test.mjs` and `integration/action-runtime-ts/backend.test.mts` already have it under the old name.

### 2.2 Rename rules

1. **Strip the marker** from identifiers, modules, files, test names, fixtures, integration folders, bridge names, diagnostics names, comments and living docs: `open05` becomes `open`, `StoreStatus05` becomes `StoreStatus`, `V05_STORE_LOCK` becomes `STORE_LOCK`.
2. **Where the stripped name collides with a different live thing in the same scope, or becomes ambiguous beside a neighbour, use a name for what the thing covers.** §2.3 lists each case. A collision the compiler finds later is resolved the same way and added to the plan's map.
3. **Rename commits are mechanical and reproducible.** The plan holds one substitution map and the command that applies it. A rename commit is: apply the map rows of its task, run the formatters, regenerate. A reviewer repeats that on the parent commit and the difference to the commit is empty.
4. **Prose follows the code, in separate commits.** Comments, test titles and living docs say "the Store file", "the sync protocol", "a Batch". They do not say "protocol 5". A prose commit changes no identifier.

### 2.3 Names that are not a plain strip

| Today | Becomes | Why |
| --- | --- | --- |
| `axton_protocols::sync`, imported `as v05` | module `axton_protocols::wire`, imported plainly; `axton_client::wire` | The prefix is needed because wire-form and in-memory types share names (`RecordKey`, `Operation`). An alias is a second name for one thing. Files: `src/sync.rs`, `src/sync/`, `tests/sync.rs`, `docs/engineering/architecture/protocols/sync.md`. No `wire` module or type exists; two test helpers named `wire` are renamed first. |
| `crates/client/src/sync05/` | `sync/` | Free once the protocols module is `wire`. |
| `crates/client/src/store05.rs` | `store_file.rs` | `store.rs` is the storage contract. This file is the Store file: tables, open, descriptors, upgrade. |
| `crates/server/src/protocol_v05.rs` and its test | `batch_entry.rs` | The Batch entry points the host binding calls, and the host adapters they run through. `mutation_batch.rs` stays the member execution. |
| `Publication05` | `PublicationHost` | It is a `Host` adapter. |
| `HostRequest::Protocol05`, `Protocol05Operation`, op `"protocol05"`; exports `Protocol05Request`, `Protocol05Context`, `Protocol05Operation` | `HostRequest::Sync`, `SyncOperation`, op `"sync"`; `SyncRequest`, `SyncContext`, `SyncOperation` | The envelope carries the sync protocol's storage and hook operations. |
| Live action `"pullV05"`, `pull05Live` | `"pull"`, `pullLive` | No other pull action exists (`packages/backend/server/connection/live.mts:17-20`). |
| `bindings/common/tests/protocol05_runtime.rs` | `runtime_network.rs` | `runtime.rs` beside it covers the bridge commands; this one drives the runtime with a network. |
| `crates/sqlite/tests/protocol05_binding.rs`, `protocol05_progress.rs` | `stream_binding.rs`, `delivery_progress.rs` | `binding` and `progress` alone do not say what is bound or progressing. |
| `crates/sim/tests/protocol05_coverage.rs` | `scenario_coverage.rs` | It checks which scenarios the simulation covers. |
| `integration/persistence/server/protocol-v05-{batch,delivery,drivers}.test.mjs` | `batch-entry.test.mjs`, `sync-delivery.test.mjs`, `sync-drivers.test.mjs` | `persistence-batching.test.mjs` and `driver-runtime.test.mjs` sit beside them. |
| `packages/frontend/dart/test/protocol5_transport.dart` | `fake_transport.dart` | Test support, beside `fake_carrier.dart`. |
| `integration/v05-sdk/`, package `axton_v05_sdk_fixture`, docs fence `title="v05-sdk"` | `integration/sdk/`, `axton_sdk_fixture`, `title="sdk"` | 41 files name the path, among them `scripts/test.sh:34-35` and `website/scripts/check_examples.py:40`, `:184`. |
| `fixtures/protocol/0.5.json` | `fixtures/protocol/wire.json` | Loaded only by `crates/protocols/tests/sync.rs:498`, `:508`. |
| `crates/compiler/tests/v04_facade.rs` | `facade.rs` | A protocol-4 marker on a test of the current generated facade. |
| `docs/engineering/architecture/client/storage/protocol5.md`, `architecture/server/protocol5.md` | `store-file.md`, `batches.md` | Named for their subject. |
| `docs/engineering/protocol5-adoption.md` | split | Its release gates (the installed check on packed bytes, the capacity run, the mobile artifact smoke tests) move into `releasing.md`. Its statement of what an unsupported SQLite file gets moves into `store-file.md`. The protocol-5 candidate record that remains moves to `docs/engineering/history/`. |

Links from frozen specs, plans and history to a moved or renamed file break. They are not repaired: those documents record the names of their time.

### 2.4 What does not change

These are real version numbers or persisted identity.

| Kept | Where | Why |
| --- | --- | --- |
| Hash domains `axton:materialization:5`, `axton:mutation-descriptor:5`, `axton:delivery-unit:5`, `axton:delivery-part:5`, `axton:delivery-plan:5`, `axton:stored-descriptor:5`, `axton:batch-intent:2` | `crates/protocols/src/sync/mutation.rs:36`, `:165`, `:197`; `sync/delivery.rs:379-390`; `crates/client/src/store05.rs:130` | Each is hashed into an ID a Store or a server has saved. |
| SQLite `format=5` and every SQLite table, column and trigger name | `crates/client/src/store05.rs:15-33` | The file layout. None carries the marker. A file older than format 5 still gets "unsupported Store format" (`store05.rs:68-76`). |
| PostgreSQL object names | `packages/backend/postgres/migration.sql` | None carries the marker; only the TypeScript constants `V05_*` and test schemas do. |
| `PROTOCOL = 6` and every JSON field name on the wire | `crates/protocols/src/sync.rs:8` | The wire. |
| HTTP routes `/sync/handshake`, `/sync/pull`, `/sync/mutations`, `/sync/fetch`, `/sync/actions`, `/sync/live` | client and server connection code | The wire. The routes keep `sync` when the Rust module becomes `wire`. |
| Tracing targets `axton::client`, `axton::server`, `axton::sqlite`; error `code` strings | all crates | None carries the marker. |
| `docs/engineering/history/**`, `docs/superpowers/**`, `CHANGELOG.md`, `integration/performance/baselines/**` | | Records of the past. |

Renamed although they cross a boundary:

- **The bridge `open` message** loses `protocol` (§2.1). A 0.5.x SDK cannot meet the new core: the npm addon is pinned exactly, and the Dart build hook downloads the core of its own version (`packages/frontend/dart/hook/build.dart:110`).
- **The native entry points** (`bindings/node/src/server.rs:208-312`). `@axtonjs/native` is its own package, pinned exactly by `@axtonjs/server`.
- **The host op `"sync"` and the exported types** `SyncRequest`, `SyncContext`, `SyncOperation` (`backend.mts:46-48`). The op reaches `@axtonjs/postgres`, which pins `@axtonjs/server` exactly, and any custom `Database` or `Persistence` adapter an app wrote. 0.6.0 already requires upgrading these packages together.
- **Diagnostics names that reach an app's records**: spans `server.handshake05`, `server.processDelivery05`, `server.processRead05`, `server.processLive05` (`backend.mts:326`), `operation="native.process_delivery05"` and its three siblings (`bindings/node/src/server.rs:240-321`), and `host.handleBootstrap05` (`host.mts:412`). Cost: a stage breakdown compared against the kept pre-rename performance baselines loses those rows.

### 2.5 The backend option

`BackendOptions.protocol5` is flattened (user, 2026-11-12). Its four members move to the top level: `authorizeStream` (required), `projectionGeneration`, `materializations`, `experimentalMutationGroupSize`. None clashes with an existing option (`backend.mts:201-262`; `admit` is the connection hook and stays). The generated `Options<Tx>` stops special-casing the nested object. `createBackend` throws at startup when `authorizeStream` is missing; today such a backend starts and then refuses every Stream. This is the one behavior that changes, and only for a backend that could not serve a client.

`Config.protocol5: Option<ProtocolConfig>` in the server crate follows: `projection_generation` (default `"1"`) and `materializations` (default empty) become fields of `Config`, and `ProtocolConfig` is deleted. An absent value already meant those defaults (`crates/server/src/delivery_plan.rs:113-117`, `mutation_batch.rs:360-374`).

This is a breaking change to the backend API. It lands before 0.6.0 is published. The commits that flatten the options and that rename the host op and the exported types carry a `BREAKING CHANGE:` footer naming the flat options, the `"sync"` op and the renamed exports, because Release Please builds the changelog from those footers. Capso pins 0.5.8 and is not touched here.

## 3. Changes

The plan carries the full substitution map. By area:

- **`crates/protocols`**: module `sync` to `wire`; `HostRequest::Sync` and `SyncOperation`; `OPERATIONS`; fixtures `wire.json` and `host-operations.json`.
- **`crates/server`, `bindings/node`, `packages/backend`**: §2.1 deletions; `batch_entry.rs`; entry points, diagnostics names and SQL constants; `Config` fields; the flat options; the generated backend types in `crates/compiler/src/emit.rs`.
- **`crates/client`, `crates/sqlite`, `crates/sim`, `bindings/common`, `bindings/mobile`, `bindings/dart`**: §2.1 deletions; `store_file.rs`, `settlement.rs`, `sync/`; every marked method and type; the open message.
- **`packages/frontend`**: the open message; test file names.
- **`integration/`, `examples/`, `scripts/`, `website/`**: `integration/sdk/`; every backend built with the flat options, `integration/performance/fixture/server.mjs` included; regenerated files, also the two the gate does not regenerate (`integration/performance/fixture/generated/` through `integration/performance/run.sh`, `integration/platform/react-native/generated/` through its `generate.sh`); temporary-directory prefixes and test schema names.
- **Docs**: 74 living files; the adoption note is split (§2.3).

## 4. Decisions

- **Delete the protocol-4 leftovers before renaming.** Rejected: renaming them too. `run05` beside a dead `run` would become two functions with unrelated names for one job.
- **Strip the marker; do not replace it with a new one** such as `v6`. The wire number already lives in `PROTOCOL`.
- **Rename the protocols module to `wire` instead of aliasing it** (user, 2026-11-12). Rejected: an alias `wire` over a module named `sync`, which keeps two names for one thing. Rejected: keeping `axton_protocols::sync` and giving the client module another name such as `link/`. Cost: the module also holds rules that are not wire forms (`target_disposition`, `settlement_ready`, `materialization_id`).
- **Flatten `protocol5` into `BackendOptions`** (user, 2026-11-12). Rejected: renaming the nested object, which keeps a required group that looks optional.
- **Rename the envelope op to `"sync"` and keep the envelope.** Rejected for this change: flattening it. `readTracking`, `guardRecords` and `applyStreamMembers` exist both as top-level requests and inside the envelope, and the PostgreSQL adapter answers both with one function (`packages/backend/postgres/src/persistence.mts:514-525`). Removing the envelope changes the host contract's shape and is its own change.
- **Keep the `"protocol4"` refusal.** Rejected: deleting it as a protocol-4 leftover. It is the error an app with an old checked-in `backend.json` gets.
- **Keep capability probing; drop only protocol probing.** Rejected: making every optional `Native` member required, which removes real checks for commit groups and adapters.
- **Rename the diagnostics names.** Rejected: keeping them for comparison with old baselines. They are names an app reads, and they would keep the marker alive in every record.
- **Hash domains keep their `:5`.** Rejected: renumbering for tidiness, which changes every materialization ID.
- **A map and a command, not a hand edit.** Rejected: "rename commits contain only renames" as a rule checked by reading; formatters rejoin lines after shorter names, so the diff is not a pure substitution. Rejected: a blanket replace of `05`, which also hits hashes and dates.

Design review (2026-11-12) changed: the proof search uses a Perl pattern, since Apple Git's extended regex has no `\b` and the first search found nothing, and it gained two companion searches for infix markers and prose; three `open_exclusive` test sites are named as not mechanical; the unused `Client.context05` field is deleted outright and the server type is named correctly; the `"protocol4"` refusal and the capability probing stay; diagnostics names are renamed and their cost stated; the package coupling and exported types are stated and `BREAKING CHANGE:` footers required; rename commits became reproducible from a map; a Store-file fixture backs the "old file still opens" check; the adoption note is split instead of moved; ambiguous stripped names got descriptive ones.

## 5. Verification

| Check | Layer |
| --- | --- |
| The full gate passes after every task | `LC_ALL=en_US.UTF-8 bash scripts/test.sh` |
| A Store file written at `main` bd46f42, with a queued Mutation and Stream rows, opens with the same Store ID, materialization ID and queue. The fixture and test are added before anything else and pass before and after every task | `crates/sqlite/tests/fixtures/`, `crates/sqlite/tests/client.rs` |
| Each §2.1 deletion: a search shows no production caller before the delete; a test that covered only the legacy branch is deleted with it and named in the commit | per commit |
| An open message with no `protocol` field opens a Store, and neither SDK sends the field | `bindings/common/tests/`, `packages/frontend/client-js`, `packages/frontend/dart/test` |
| A backend without `authorizeStream` fails to compile against the generated types and throws at startup with hand-written options | `crates/compiler/tests/facade.rs`, `integration/persistence/server/startup.test.mjs` |
| The digests in `wire.json` and every materialization ID asserted by the suites are unchanged | `crates/protocols/tests/wire.rs`; the fixture's diff is a path change only |
| Each rename commit is reproducible: applying its map rows, the formatters and the regeneration to its parent gives the commit exactly | review of each commit (§2.2 rule 3) |
| The proof search is empty except for the recorded residue: `git grep -nP '\b[A-Za-z_]\w*05\b\|[vV]05\|[pP]rotocol_?5' -- ':!docs/engineering/history' ':!docs/superpowers' ':!CHANGELOG.md' ':!integration/performance/baselines' ':!*.lock' ':!package-lock.json'`. Today: 2,901 lines. Expected residue, to be listed by `file:line` in the PR: three integrity hashes in nested `package-lock.json` files and three element IDs in `website/docs/assets/architecture.excalidraw`, none of them a name | last task |
| Two companion searches with the same exclusions, because the proof search misses a marker inside a name and prose with a space: `'[A-Za-z_]05[A-Za-z_]'` (179 lines today; expected residue: hex in nested lockfiles, in `wire.json` digests and in `architecture.svg`) and `-i 'protocol[ -]5\b'` (135 lines today; expected residue: none) | last task |

## 6. Out of scope

- Moving the client crate's files into folders that match the architecture tree in `docs/engineering/architecture.md` (engine, storage, connection). That is the next change; names chosen here, such as `store_file.rs`, are not meant to settle it.
- Splitting `settlement.rs` by job, and the overlapping module names `queue.rs` and `mutation_queue.rs`.
- Removing the `"sync"` envelope and the three duplicated top-level host requests.
- Moving the non-wire rules out of the `wire` module.
- Capability probing on `Native` and on database adapters.
- `integration/platform/react-native/smoke.ts`, which does not typecheck on `main`.
- Capso.

## 7. Open questions

None.
