# Collaborative Mobile To-do Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The user requested a handoff to another agent; this document does not start execution or delegate work.

**Goal:** Deliver issue #31: a minimal React Native To-do app that demonstrates real AXTON collaboration on two independent iOS clients.

**Architecture:** One generated User/Todo schema, one TypeScript/Prisma/PostgreSQL backend, and one React Native app using the existing Rust/SQLite engine through an Expo native carrier. Consume the verified React Native client/native module from #100; SDK implementation is outside this demo issue. Move the old example's regression harness into integration fixtures before deleting the public example.

**Tech Stack:** React Native, TypeScript, Expo development build/local iOS module, Rust, SQLite, Node, Prisma, PostgreSQL. Existing repository versions remain unchanged unless the mobile dependency resolver requires an isolated app toolchain adjustment.

## Global constraints

- **Blocked by [React Native support #100](https://github.com/zanminwang/axton/issues/100).** Complete SDK/native integration and its iOS validation first, then execute this demo plan.

- Source of requirements: [design spec](../specs/2026-09-15-todo-mobile-design.md).
- Use React Native with TypeScript. First required target: two independent iOS simulator installations.
- Exactly two business models: `User` and `Todo`; one shared channel: `todo:demo`.
- UI actions: Add task and Done only. No assign, replies, edit/delete, filters, counters, or always-visible diagnostics.
- Use an Expo development build; do not rely on Expo Go or Metro for offline relaunch evidence.
- Keep implementation simple: no mandatory generic client factory or broad SDK refactor; extract shared helpers only for a concrete need.
- Preserve the existing Rust sync contract and Node/Dart regression coverage. No custom sync engine, periodic polling, or in-memory substitute.
- Keep repository documentation in English. Keep video production assets under `marketing/`.
- Scope excludes browser/WASM (#59/#72), Android verification, production login, deployment, and video production.
- Planning baseline: `a837cf8` on branch `codex/todo-mobile`. All unchecked items below are future implementation work, not completed validation.

## File and responsibility map

Paths below are relative to the repository root. New names are implementation targets; adapt a path only if a newer main already supplies the same responsibility, and update this plan accordingly.

| Area | Create or modify | Responsibility |
| --- | --- | --- |
| Shared example | `examples/todo/models/todo.model`, `prisma/schema.prisma`, `server.mts`, `seed.mts`, `generate.sh`, `run.sh`, `package.json`, `package-lock.json`, `README.md` | Schema, backend, launch, repeatable seed, developer guide |
| Generated code | `examples/todo/generated/node/`, `examples/todo/generated/mobile/` | Compiler output from the same schema with different runtime imports |
| Mobile app | `examples/todo/mobile/App.tsx`, `src/todo.ts`, `src/config.ts`, `src/useTodos.ts`, `src/TodoScreen.tsx`, `app.config.ts`, `metro.config.js`, `package.json`, `package-lock.json`, `tsconfig.json` | Identity/configuration, domain adapter, local watch, one screen, build configuration |
| Tests | `integration/e2e/todo.test.mjs`, `todo-run.sh`; `integration/platform/run_todo_ios_smoke.sh` | Real backend demo behavior and actual mobile application runtime |
| Legacy fixture | `integration/e2e/fixtures/round-trip/` | Existing Entry/Edit application and generated code used by regression tests |
| Migration consumers | `scripts/test.sh`, `integration/e2e/run.sh`, `round-trip.test.mjs`, `dart_client.dart`, `dart_live_client.dart`, `website/scripts/check_examples.py`, `tsconfig.json` | Keep verification runnable after path changes |
| Public docs | `website/docs/getting-started.md`, affected schema/backend/frontend pages, `website/docs/api-index.md`, `website/docs/frontend/platforms.md`, `website/mkdocs.yml` | Actual demo instructions and supported runtime/API surface |
| Marketing link | `marketing/videos/README.md` | Point existing production work to the verified example; no new video scope |

## Task 1: Confirm the integration baseline and compile the business model

**Files:** create `examples/todo/models/todo.model`, `examples/todo/generate.sh`; generated output under `examples/todo/generated/{node,mobile}`. Read `AGENTS.md`, architecture/guarantees/testing docs and the current client/binding/compiler sources.

**Consumes:** spec's exact schema and current compiler `--client-runtime` / `--backend-runtime` options.

**Produces:** generated `User`, `Todo`, `AddTodo`, `SetTodoDone`, `GeneratedClient`, `GeneratedTransaction`, and typed backend handlers. Later tasks must use actual emitted types.

- [x] Check worktree and save these planning documents in a documentation commit before rebasing. Fetch current main and rebase the isolated branch; preserve unrelated edits. Read changes to client host commands and #58's live-session implementation before adapting them.

```sh
git status --short --branch
git log -1 --oneline
git diff --name-status a837cf8..HEAD
```

- [x] Copy the exact `model User` / `model Todo` schema block from the spec into `examples/todo/models/todo.model`. Generate both targets using the repository compiler; do not edit generated outputs.

```sh
cargo run -p axton-compiler --locked -- compile examples/todo/models examples/todo/generated/node --backend-runtime ../../../../packages/server/index.mts --client-runtime ../../../../packages/client-js/index.mts
cargo run -p axton-compiler --locked -- compile examples/todo/models examples/todo/generated/mobile --backend-runtime ../../../../packages/server/index.mts --client-runtime ../../../../packages/client-react-native/index.ts
```

- [x] Put these commands in `generate.sh` with `set -euo pipefail` and a repository-root `cd`. Confirm both `schema.json` descriptors match; runtime import strings may differ in generated source. Typecheck backend inputs before writing handlers. Confirm the wire create separates identity from values, while the generated backend `AddTodoInput.todo` is the complete Todo record. Updates use `patch` server-side / `values` client-side.
- [x] Run the existing binding and compiler baseline; record failures before changing runtime code.

```sh
cargo test -p axton-binding -p axton-compiler --locked
```

Expected: existing tests pass and both generation commands exit zero. This proves schema compatibility, not mobile runtime support. Commit the schema/generation deliverable.

## Task 2: Build the reusable backend and verify real To-do synchronization

**Files:** create the example's Prisma schema, package/lockfile, `server.mts`, `seed.mts`, `run.sh`; create `integration/e2e/todo.test.mjs` and `todo-run.sh`.

**Consumes:** generated `Handlers<Prisma.TransactionClient>`, `Loaders<Prisma.TransactionClient>`, `createBackend`, Node `GeneratedClient`.

**Produces:** `createExample()` with `db`, `backend`, `schema`, `initialize(): Promise<void>`, `listen(port: number)`, and `close(): Promise<void>`, preserving the old harness's lifecycle shape. `seed(tx, backend)` creates stable demo rows and awaits publication inside the transaction. `todo-run.sh` starts/cleans a disposable PostgreSQL cluster and runs the To-do tests.

- [x] Define the two Prisma models, keeping infrastructure tables managed by the existing persistence adapter:

```prisma
model User {
  id String @id
  name String
  todos Todo[]
}
model Todo {
  id String @id
  title String
  done Boolean
  createdById String
  createdBy User @relation(fields: [createdById], references: [id])
}
```

Use the current example's PostgreSQL datasource and pinned Prisma packages. Seed `alice`, `bob`, and task IDs `seed-1`, `seed-2`, `seed-3` with the three titles from the spec. Seed with upsert/create-if-missing; ordinary startup must not reset edits. Reset tooling must target a dedicated demo database and the explicitly selected simulator containers.

- [x] Add failing real-backend tests using this generated mutation shape:

```ts
await alice.transaction(tx => tx.mutate.addTodo({
  todo: { id: "test-task", title: "  Buy milk  ", done: false, createdById: "alice" },
}));
await alice.transaction(tx => tx.mutate.setTodoDone({
  todo: { identity: { id: "test-task" }, values: { done: true } },
}));
```

Assert after settlement: PostgreSQL and Bob both contain one `test-task`, title `Buy milk`, done `true`, creator `alice`; both queues are drained. Use bounded eventual assertions and watch callbacks following the existing E2E harness, not fixed sleeps as evidence.

- [x] Implement handlers against their emitted types. The validation core is:

```ts
function titleForInsert(title: string): string {
  const value = title.trim();
  if (!value) throw new MutationRejected("todo.title_empty");
  return value;
}
function validateCreate(userId: string, values: { createdById: string; done: boolean }): void {
  if (values.createdById !== userId) throw new MutationRejected("todo.creator_invalid");
  if (values.done !== false) throw new MutationRejected("todo.initial_state_invalid");
}
```

Insert through `tx.todo.create`; translate only a proven task-primary-key conflict into `todo.id_conflict`. For completion, update only `done` and translate a missing row to `todo.missing`. Use `notify({ channel: "todo:demo", records: [input.todo] })` in each handler. The loader returns rows in requested ID order, including null for missing rows.

- [x] Restrict the development authentication callback to tokens `alice` and `bob`. Validate loader user/channel scope, and reject creator spoofing in the handler. Do not present this allowlist as production login.
- [x] Add tests for whitespace rejection, invalid creator, initial done=true, unknown identity, missing task, same-ID distinct create, duplicate frozen-request retry, offline add-then-done, and two opposing completion operations accepted in each controlled commit order. Include a second client adding a different task while the first is offline. Verify rollback and persisted receipts through PostgreSQL.
- [x] Run `bash integration/e2e/todo-run.sh`. Expected: all named scenarios pass through real Rust clients, real HTTP/WebSocket, and PostgreSQL. Commit the backend and automated scenario harness.

## Task 3: Consume the verified React Native integration

**Prerequisite:** [#100](https://github.com/zanminwang/axton/issues/100) must deliver its supported API and iOS runtime evidence first. Its implementation is tracked in the [separate SDK plan](2026-09-15-react-native-support.md).

- [x] Confirm #100's verified revision, generated runtime import path, native module installation steps, and supported API surface.
- [x] Scaffold the To-do Expo app using that integration's documented dependency versions and build configuration.
- [x] Install/reuse the native module and client package. Do not implement a second carrier, transport, or generic runtime in this demo.

## Task 4: Verify demo integration with the supported SDK

- [x] Generate the Todo mobile client against #100's runtime entry point and typecheck/bundle the app.
- [x] Run a local open/query/close through the installed SDK and verify the generated mutation imports resolve.
- [x] If a missing SDK behavior prevents integration, report it against #100; do not silently expand this demo into SDK development.

## Task 5: Implement the minimal screen on the real client

**Files:** `mobile/src/config.ts`, `todo.ts`, `useTodos.ts`, `TodoScreen.tsx`, `App.tsx`; app config/build scripts and README.

**Consumes:** generated mobile `GeneratedClient`, `Todo` / `User` types, #100's documented native path API, an Expo-compatible UUID generator, and Task 2's backend.

**Produces:** one screen per installed application; real local watches and writes. Define the application domain interface in `src/todo.ts`:

```ts
import type { Todo } from "../../generated/mobile/generated";
export interface TodoSession {
  watch(listener: (rows: Todo[]) => void, onError: (error: unknown) => void): () => void;
  add(title: string): Promise<void>;
  setDone(id: string, done: boolean): Promise<void>;
  close(): Promise<void>;
}
```

- [x] Validate launch configuration for backend URL and user `alice` or `bob`; default Alice for a single simulator and document Bob configuration. Open a stable database path, subscribe to `todo:demo`, and watch both the selected User and Todo records. Never regenerate the database/client identity on component rerenders.
- [x] Implement `TodoSession` using generated transaction methods. Generate the ID once per user submission with the selected Expo-compatible UUID API. Use the exact add/setDone call shapes from Task 2; return only after local commit. Sort emitted rows by code-unit ID order, independent of locale. Dispose watches and connection on teardown; foregrounding wakes the existing connection rather than creating a second one.
- [x] Build the spec's single screen with React Native primitives: avatar/name header, To-do heading, scrollable checkbox rows, persistent add input/plus button. The handler contract is:

```ts
async function submit() {
  const title = draft.trim();
  if (!title || submitting) return;
  setSubmitting(true);
  try {
    await session.add(title);
    setDraft("");
  } catch (error) {
    setError(error instanceof Error ? error.message : "Could not add task");
  } finally {
    setSubmitting(false);
  }
}
```

Bind the checkbox to `session.setDone(todo.id, !todo.done)`. Use an accessible checkbox role/state and native keyboard submission. Keep task state sourced from local watch callbacks. Avoid component snapshot tests that merely mirror JSX; verify empty/long titles, keyboard overlap, and touch targets in the running app.
- [x] Wire initial load, cached offline start, local commit error, and persisted rejection feedback. Use actual engine status/rejections; do not treat every transport error as a failed task write or display internal machine codes to the user. Keep technical logs in the harness.
- [x] Run the app with Alice and Bob against the same backend. Add on Alice, complete on Bob; observe updates in both. Check that no assignment/reply/delete controls exist. Commit the screen and domain adapter.

## Task 6: Prove two-phone offline recovery and document the runnable demo

**Files:** `integration/platform/run_todo_ios_smoke.sh`, diagnostic assertions in `mobile/src/smoke.ts` reachable only in test configuration, `examples/todo/README.md`.

**Consumes:** complete app/backend, two simulator UDIDs, dedicated demo database, bundled-JavaScript app build.

**Produces:** a reproducible runner that fails on incorrect results and records device/build/version evidence. Diagnostic commands live outside the normal UI and use the same generated client/session as the screen.

- [x] Build a simulator app with embedded JS. Install the same app independently on two disposable simulators. Accept UDIDs as runner arguments, verify they differ, select separate user launch configuration, and verify distinct stored `clientId` values. Do not assume a fixed simulator model or Xcode version.
- [x] Drive the named sequence: seed/load → Alice add → Bob done → disconnect Alice → Alice add and done → Bob independent add → terminate/relaunch Alice offline → reconnect → assert all three stores agree and no pending mutations remain. Include a backend restart with persistent PostgreSQL and a lost-response retry in automated integration coverage.
- [x] Make the per-client disconnect real for at least one run, using an isolated proxy/process or network fault harness scoped to that simulator's backend route. SDK pause/resume is a useful additional test, but does not establish network-loss handling. Do not alter the user's system-wide firewall or unrelated network sessions.
- [x] Use `xcrun simctl terminate` / `launch` for process restart and capture screenshots from both devices at online, offline/relaunch, and settled stages. Await explicit diagnostic assertions with a timeout; timeout is a test failure. Clean up only simulators/processes created by the runner.
- [x] Document exact executable commands implemented by the earlier tasks:

```sh
bash examples/todo/generate.sh
bash examples/todo/run.sh
# In another terminal, from examples/todo/mobile:
npm ci
npm run ios
# From repository root, with two caller-selected simulator IDs:
bash integration/platform/run_todo_ios_smoke.sh "$ALICE_SIMULATOR_UDID" "$BOB_SIMULATOR_UDID"
```

Document backend URL selection, seed/reset isolation, identity configuration, required Xcode/Rust targets, native rebuild versus JS refresh, and embedded-JS test build. Record actual tool versions, revision, commands, results, and limits. This plan does not claim those commands have already been implemented or run.
- [x] Explain the two tables and two generated operations in the README, followed by the backend notification and frontend watch. Link engine-owned metadata docs. Commit the runtime evidence and runnable guide after this gate passes.

## Task 7: Retire the old public example without deleting its regression value

**Files:** move old example into `integration/e2e/fixtures/round-trip/`; update all migration consumers and affected public documentation from the file map.

**Consumes:** working To-do example and existing round-trip assertions.

**Produces:** `examples/todo` as the runnable public example, preserved fixture coverage, no broken old paths.

- [x] Inventory consumers before moving anything:

```sh
rg -n 'rust-round-trip|Entry|Edit' examples integration/e2e scripts website
```

Classify Entry/Edit references: retain generic API examples that need their richer fields, but migrate onboarding commands and To-do quickstart to the new app. Do not blanket-replace model names.
- [x] Move the old files into the integration fixture. Adjust backend/client package imports to the new depth, regenerate fixture clients with correct runtime imports, and update JS/Dart test fixture paths. Update fixture-local npm lockfile metadata only if needed. Keep all original behavioral assertions.
- [x] Update `scripts/test.sh`, both E2E runners, root typecheck inclusion and snippet checking. Let focused Entry snippets compile against the relocated fixture and To-do quickstart snippets against the new generated client. Remove the old public directory only once those consumers resolve.
- [x] Update getting-started/schema/backend/frontend guides, platform matrix and API index with actual supported mobile APIs and setup. Link the mobile example from `marketing/videos/README.md`; leave video production issues separate. Add no browser capability claim.
- [x] Run the affected host gates, then the complete host gate once:

```sh
bash integration/e2e/run.sh
bash integration/e2e/todo-run.sh
python3 website/scripts/check_examples.py
bash scripts/test.sh
```

Run the strict website build using `website/README.md`. Expected: existing and new behavioral assertions pass; documentation snippets compile; no stale runnable paths. Historical issue/spec references can still name `rust-round-trip`. Commit the migration.

## Final review and handoff report

- [ ] Map spec criteria A1–A9 to actual tests and artifacts. A1/A3/A4/A5 need real iOS execution; Node tests alone cannot satisfy them.
- [x] Inspect the final diff for dropped regression assertions, generated files edited by hand, task controls outside scope, credentials, temporary database files, and build outputs.
- [ ] Update #31 with concrete validation results and remaining limitations. Keep #31 and #59 as blockers for #72; do not close browser issues because the mobile demo works.
- [ ] Report what changed, executed commands, UI/runtime evidence, unsupported targets, and remaining work. If simulator/toolchain access is missing, finish independent host work and report the mobile acceptance gap explicitly; do not mark the mobile demo complete.

## Spec coverage map

| Criteria | Tasks |
| --- | --- |
| A1 | 3, 5, 6 |
| A2 | 1, 2 |
| A3 | 2, 4, 5, 6 |
| A4–A5 | 2, 4, 6 |
| A6 | 2, 6 |
| A7 | 2, 5 |
| A8 | 4, 7 |
| A9 | 6, 7 |
