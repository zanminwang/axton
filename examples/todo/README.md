# Collaborative To-do

Two phones, one shared list. Alice and Bob each run the same React Native app against their own local SQLite database; the app writes locally first and synchronizes through an application-owned TypeScript backend on PostgreSQL. This is the example behind [#31](https://github.com/zanminwang/axton/issues/31).

The app has two actions: add a task and mark it done. Everything else (assignment, replies, editing, deleting) is deliberately absent so the schema, backend and screen stay readable.

## Layout

| Path | Owns |
| --- | --- |
| `models/todo.model` | The schema: `User`, `Todo`, `AddTodo`, `SetTodoDone` |
| `generate.sh` | Compiles the schema into `generated/node` (Node backend and tests) and `generated/mobile` (React Native) |
| `prisma/schema.prisma`, `server.mts`, `seed.mts` | Backend tables, handlers, loaders, development authentication and the seed data |
| `run.sh` | Builds, generates, starts a disposable PostgreSQL cluster and the backend on port 4242 |
| `mobile/` | The Expo app: one screen, a session adapter over the generated client, launch configuration |
| `../../integration/e2e/todo.test.mjs` | Backend scenarios through real Rust clients, HTTP/WebSocket and PostgreSQL |
| `../../integration/platform/run_todo_ios_smoke.sh` | Two independent simulators: live edits, lost response, offline add-then-done, relaunch, reconnect |

## Prerequisites

macOS with Xcode and an iOS simulator runtime, CocoaPods, Node, Rust with the `aarch64-apple-ios-sim` target, and PostgreSQL command-line tools (`initdb`, `pg_ctl`). The app pins Expo 57, React Native 0.86.3 and React 19.2.3 in `mobile/package-lock.json`; the root toolchain is unchanged.

## Run the backend

```sh
bash examples/todo/generate.sh
bash examples/todo/run.sh
```

`run.sh` creates a temporary cluster, seeds Alice, Bob and three tasks, and listens at `http://127.0.0.1:4242`. Stop it with Ctrl-C; the cluster is deleted. To keep data between runs, start the backend yourself with `DATABASE_URL` pointing at your own database:

```sh
(cd examples/todo && npm ci && npm run generate)
DATABASE_URL=postgresql://... PORT=4242 node examples/todo/server.mts
```

Seeding is create-if-missing, so an ordinary restart keeps edits. Reset by dropping the database (or the temporary cluster) and starting again.

## Run two phones

Build the Rust simulator slice and the app once:

```sh
rustup target add aarch64-apple-ios-sim
bash packages/client-react-native/native-module/scripts/build-ios.sh simulator
cd examples/todo/mobile
npm ci
npx expo prebuild --platform ios --no-install
(cd ios && pod install)
npm run ios:build
```

`ios:build` produces `ios/build/Build/Products/Release-iphonesimulator/AXTONTodo.app` with embedded JavaScript. `npm run ios` (a development build with Metro) also works for iterating on the screen; Expo Go cannot load the native module.

Identity is launch configuration, not a login: without configuration the app is Alice on `http://127.0.0.1:4242`. To make a second simulator Bob, write `config.json` into that installation's Documents directory and launch:

```sh
udid=<simulator udid>
xcrun simctl install "$udid" ios/build/Build/Products/Release-iphonesimulator/AXTONTodo.app
docs="$(xcrun simctl get_app_container "$udid" dev.axton.Todo data)/Documents"
mkdir -p "$docs" && echo '{"user":"bob","url":"http://127.0.0.1:4242"}' > "$docs/config.json"
xcrun simctl launch "$udid" dev.axton.Todo
```

Each installation keeps its own database file and client identity under Application Support, so two simulators are two independent phones. Add a task on one; it appears on the other after the backend accepts it. Mark it done on the other; both converge.

## Walkthrough

1. **Two tables.** `models/todo.model` declares `User(id, name)` and `Todo(id, title, done, createdById)` with `createdBy` as a reference. The backend adds no other application tables; AXTON's own sync tables come from `packages/postgres/migration.sql`.
2. **Two operations.** `AddTodo` creates a task; `SetTodoDone` updates only `done`. The compiler emits typed builders for the app and typed `Handlers`/`Loaders` for the backend from the same file, so the wire shapes cannot drift.
3. **Backend rules.** `server.mts` trims the title and refuses empty ones, requires the creator to be the authenticated user and `done` to start false, and turns only a proven primary-key collision into `todo.id_conflict`. Each handler publishes the changed record on channel `todo:demo` inside the same database transaction, so a notification never precedes its data.
4. **Local watch.** `mobile/src/todo.ts` opens the generated client on a per-user database, subscribes to `todo:demo`, and exposes `watch`, `add` and `setDone`. `add` and `setDone` return after the local commit; the screen renders from watch callbacks only, never from a second in-memory store.
5. **Offline and back.** Without a network, adds and completions commit locally and queue in SQLite. Killing and relaunching the app keeps the rows, the queue and the client identity. On reconnect the engine pushes the queued create before its dependent update, catches up missed changes over HTTP, then follows the live stream. `integration/platform/run_todo_ios_smoke.sh` proves this on two simulators with a real per-phone network fault.

Engine-owned tables, cursors and receipts are described in [client storage](../../docs/engineering/architecture/client/storage/README.md) and the [guarantees](../../docs/engineering/guarantees.md).

## Tests

```sh
bash integration/e2e/todo-run.sh
bash integration/platform/run_todo_ios_smoke.sh
```

The e2e runner covers the happy path, every rejection code, an unknown identity, a lost push response, a backend restart on the same database, offline add-then-done across a reopen, and opposing completions in both commit orders. The simulator runner needs the Release app above; it creates two disposable simulators (newest installed iOS runtime and first iPhone device type by default, or `AXTON_TODO_SIM_RUNTIME`/`AXTON_TODO_SIM_DEVICE`) and deletes only those.

## Evidence

Verified 2026-09-15 on branch `codex/todo-mobile` on the working tree later committed as `bd1678c` and rebased onto `c54ff70` (React Native support, #100); the SDK follow-up between those commits changed only build tooling and test timeouts with Xcode 26.5 (17F42), the iOS 26.5 simulator runtime (23F77), two disposable iPhone 17 simulators, Node 26.4.0, cargo 1.98.1, CocoaPods 1.16.2, Expo 57.0.22, React Native 0.86.3 and React 19.2.3.

```sh
bash integration/e2e/todo-run.sh                      # 15 scenarios, all passed
bash integration/platform/run_todo_ios_smoke.sh       # PASS: two To-do phones, lost-response retry, offline add-then-done, process restart and convergence
```

The Release app embedded `main.jsbundle` (1.5 MB, no Node built-ins, N-API binary or Node `ws`) and linked the Rust carrier. Phase results recorded by the app on each simulator:

| Phase | Result |
| --- | --- |
| alice/bob online | both loaded the seeded users and three tasks; Alice's add appeared on Bob live; Bob's completion appeared on Alice live; queues drained |
| alice offline | her proxy answered 503 and refused upgrades; a new task and its completion committed locally and showed through the watch; 2 pending |
| alice restart | after `simctl terminate`/`launch` while still disconnected: same client ID, task still done, 2 pending |
| bob remote | Bob added a task while Alice was disconnected |
| alice settle, bob observe | both converged to the six PostgreSQL rows with 0 pending |

Product screen: with the same backend, Alice and Bob were each launched without a test phase on their own simulator (one at a time on a memory-limited host); both showed the avatar header (A on blue, B on green), the To-do heading, the same three seeded tasks, and only the checkbox rows, the add field and the plus button. The SDK harness scenario was also rerun on the final SDK commit and passed.

Backend after the run: five handler executions (three adds, two completions) despite one deliberately dropped push response; Alice's and Bob's client IDs differed. Screenshots and JSON assertions were written to the printed evidence directory.

Limits: arm64 iOS simulator, app in the foreground. Physical devices, Android, background delivery while iOS suspends the app, and the x86_64 simulator slice are not covered. The web version is tracked separately in #72.
