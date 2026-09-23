# React Native Support Implementation Plan (#100)

> **For agentic workers:** Use superpowers:executing-plans. This is now a continuation plan: implementation exists but final native acceptance is incomplete. Checked steps are complete; unchecked steps may contain partial implementation and require their remaining verification.

**Goal:** Deliver [React Native support #100](https://github.com/zanminwang/axton/issues/100) independently, then unblock [To-do demo #31](https://github.com/zanminwang/axton/issues/31).

**Architecture:** Reuse Rust RuntimeHost/SQLite and the generated TypeScript client contract through a small native carrier and React Native host/transport adapter.

**Tech stack:** React Native, TypeScript, Expo development build, Rust, SQLite, and the existing Node backend test harness.

**Specification:** [React Native support](../specs/2026-09-15-react-native-support.md). **Working state and evidence:** [Handoff](../handoffs/2026-09-15-react-native-support.md).

## Resume here

Reuse `/Users/stevewang/Github/local first state/.worktrees/react-native` on `codex/react-native`. All changes are uncommitted; a new checkout will miss them. Do not restart implemented components.

- [x] Check branch/status and disk capacity. The last native build and aggregate host gate stopped on disk exhaustion. Preserve unrelated files; resume heavy work only after capacity is available.
- [x] Check native assertions against the spec, including failed-open and closed-handle cases. Add a focused failing test before implementing any missing behavior.
- [x] Complete the Release simulator build using the commands below. The path-with-spaces Expo Constants fix already exists as a repeatable config plugin.

```sh
node --test packages/client-react-native/plugins/expo-path-spaces.test.cjs
cd integration/platform/react-native
./node_modules/.bin/tsc --noEmit
npx expo prebuild --platform ios --no-install
(cd ios && pod install)
npm run ios:build
```

Expected: plugin/type checks pass, AxtonNative autolinks, and the Release app builds with embedded JS. Follow the [harness setup](../../../integration/platform/react-native/README.md) only for missing prerequisites; do not recreate intact artifacts.

- [x] From the repository root, run the real two-simulator scenario with the successful build's actual app path:

```sh
AXTON_RN_APP_BUNDLE=/absolute/path/from/build/axtonrnharness.app \
  bash integration/platform/run_react_native_ios_smoke.sh
```

Expected: all phase assertions pass; Alice's identity and offline create/dependent edit survive relaunch; both clients match PostgreSQL with no pending work; backend handlers execute one add/four edits despite the dropped response. Inspect JSON evidence and screenshots. Fix any actual failure with a focused regression before rerunning affected checks.

- [x] Run `CARGO_INCREMENTAL=0 bash scripts/test.sh` once adequate space is available. Expected: exit zero. Earlier aggregate runs did not pass.
- [x] Update evidence and audit package/platform claims, including the premature README statement that foreground execution is tested. Record actual revision/toolchain and unsupported targets.
- [x] Check documentation links and `git diff --check`; obtain whole-branch review. Prior bounded reviews did not establish native runtime correctness.
- [x] Address findings, rerun affected checks and commit only intended source/docs. Exclude build outputs, Pods, node_modules and simulator data. Report #100 ready to unblock #31 only after all acceptance gates pass.

## Scope and constraints

- First required target is iOS. Preserve Node/Dart behavior; no browser/WASM or Android support claim.
- Keep implementation simple; shared abstractions serve concrete needs, not a broad SDK redesign.
- Build a reusable native module under `packages/client-react-native/native-module/` and an integration-only app under `integration/platform/react-native/`. Do not place required SDK functionality inside the To-do example.
- Use the existing Entry/Edit test schema/backend as integration fixtures, generating an RN runtime entry point with the compiler's `--client-runtime` option. Read the generated ports and binding architecture before implementation.
- The harness must establish real native storage, transaction, network and restart behavior without the To-do product UI.
- Do not replace `examples/rust-round-trip` or implement To-do tables/UI here. Those are #31 responsibilities.
- These tasks were extracted from the original combined plan. The scope above overrides their former placement in the demo app.

## Task 1: Add the iOS carrier and prove persistent native calls

**Files:** create `bindings/mobile/*`, `packages/client-react-native/native-module/*`, and a minimal Expo harness under `integration/platform/react-native/`; modify root Cargo workspace/lockfile.

**Consumes:** `RuntimeHost::call(serde_json::Value) -> Result<serde_json::Value>` and the client JSON contract in the binding architecture document.

**Produces:** Expo module `AxtonNative` with `clientCall(request: string): Promise<string>` and `databasePath(name: string): Promise<string>`. Successful `clientCall` returns the unwrapped RuntimeHost response JSON, like Node's carrier; Rust/ABI failures reject. The database path is persistent and stable across launches.

- [x] Scaffold a blank TypeScript Expo app and local iOS module using the official [Expo local-module guide](https://docs.expo.dev/modules/get-started/). Resolve the compatible Expo/React Native/React set once, pin it in the app lockfile, and record versions and Xcode/iOS target in the README. Do not guess current versions or upgrade the root workspace as part of scaffolding.
- [x] Create a `staticlib` Rust crate using existing workspace dependency conventions. Start from the small C carrier in `bindings/dart/src/lib.rs`, with symbols renamed `axton_mobile_call` and `axton_mobile_free`. Keep panic catching, null/UTF-8/JSON validation, and the process host mutex. This adds a carrier; it must not alter Dart's ABI. Publish this header:

```c
#ifndef AXTON_MOBILE_H
#define AXTON_MOBILE_H
char *axton_mobile_call(const char *input);
void axton_mobile_free(char *output);
#endif
```

- [x] Bind the C carrier through an Expo Swift `AsyncFunction` executed on a dedicated serial background queue. Copy the output string before `axton_mobile_free`; use `defer` to free it on every decode/error path. Decode `{ok,result,error}` and return serialized `result` only when `ok` is true. Do not expose the C pointer or block the main UI thread. The TS-facing contract is:

```ts
export interface AxtonNativeModule {
  clientCall(request: string): Promise<string>;
  databasePath(name: string): Promise<string>;
}
```

- [x] Implement `databasePath` using Application Support; accept a basename only and create its directory. Use one stable name per configured demo user. Add iOS static-library build/link settings and Expo module autolinking; support the selected simulator architecture first. Document device slices as unverified until built and exercised.
- [x] In a diagnostic harness inside the native app, send `open`, local transaction/create, `commit`, `close`, `open`, and `query`; assert the row survives. Test malformed JSON, failed open, rollback, and closed-handle rejection. Terminate/relaunch once with a committed row. These assertions must invoke the actual carrier rather than a JS mock.
- [x] Build and run using the generated local app's `ios` command, backed by `expo run:ios`. Expected: linked Rust library, successful native promise calls, persistent row after relaunch, no leaked output buffers in inspected error paths. Commit the carrier/scaffold with actual command/toolchain evidence.

## Task 2: Make the generated TypeScript client usable on React Native

**Files:** `packages/client-react-native/*` and existing `packages/client-js/*` helpers only as needed; tests in `integration/bindings/client-react-native/`.

**Consumes:** native carrier from Task 1, existing generated runtime surface, current Node connection driver and HTTP/live contracts.

**Produces:** mobile `Client` plus `Connection`, `ConnectionOptions`, `ServerOptions`, and `RecordValue` exports required by generated `client.ts`. No generic client-class factory is required. Node still exports its original API; mobile explicitly documents its supported subset.

- [x] Inspect the current Node client and implement only the platform boundary required by the generated mobile client. Reuse existing platform-neutral connection/transport helpers directly. Extract a small shared function only when both entry points need it; preserve Node APIs and behavior. Do not copy the entire SDK or make a generic runtime framework a prerequisite.

The native carrier already defines the required string boundary:

```ts
export type NativeCall = (request: string) => Promise<string>;
```

Keep native-call wiring, transaction scope, and RN network adaptation explicit in the mobile adapter. If newer main already provides suitable shared code, use it. A `ClientPlatform` abstraction or `createClientClass` factory is an implementation option only if it demonstrably reduces the necessary change; neither is an acceptance requirement.

- [x] Reuse JSON serialization validation and local change notification behavior without importing Node-only modules. Add a small listener set only if the mobile adapter needs one; emit over a snapshot so unsubscribe during an event is safe. Ensure generated mobile imports never load `node:async_hooks` transitively.
- [x] Implement mobile transactions with an explicit object scope, serialized command queue, closed flag, outstanding-operation count, and first-failure retention. Copy the existing transaction queue/finish rules, not its AsyncLocalStorage mechanism. Prefix its native operations with `transaction: true`. The client's exclusive queue brackets callback execution with begin/commit or rollback. Do not expose raw nested savepoints in the mobile adapter. Node's savepoint support and types must remain intact.
- [x] Add transaction tests: thrown callback rolls back; caught native failure still prevents commit; forgotten await prevents commit; queued operations drain before rollback; retained transaction object rejects after finish; concurrent top-level transactions serialize; external reads cannot see partial writes. Run the same generated read/mutate usage through the mobile port.
- [x] Implement RN HTTP via native fetch and WS via native WebSocket. Preserve bearer auth on both paths, subscribe acknowledgement before catch-up, abort on close/pause, bounded page buffering, overflow-triggered catch-up, and generation checks. RN sockets lack `ws.pause/resume/terminate`; use their actual close/event API, invalidate late callbacks, and keep recovery bounded. Do not polyfill `ws` or buffer an unlimited stream.
- [x] Add transport tests for ack ordering, cancellation during token resolution and catch-up, stale pages after subscription changes, duplicate/overlapping pages, buffer overflow, and reconnect. Verify HTTP is idle after initial catch-up during ordinary streamed updates. If #58 is implemented at execution time, drive the Rust live commands instead of preserving obsolete host logic.
- [x] Run the adapter tests, existing Node tests, and generated type checks:

```sh
npm run typecheck
node --test integration/bindings/client-js/*.test.mjs
node --test integration/bindings/client-react-native/*.test.mjs
bash integration/generated-api/verify.sh
```

Add a React Native Metro bundle check as part of the integration harness build: there must be no imports of `node:*`, the N-API binary, or the Node `ws` package. Expected: old behavior preserved and mobile generated code typechecks/bundles. Commit this separately from UI changes.

## Task 3: Verify the supported runtime before unblocking the demo

- [x] Create `integration/platform/run_react_native_ios_smoke.sh` using two caller-selected, independently installed simulator instances and a disposable backend test database.
- [x] Verify generated queries/watches/mutations, rollback, failed native calls, and stable client IDs with real SQLite.
- [x] Establish actual HTTP/WebSocket collaboration between both instances. Verify authentication, cancellation, reconnect, catch-up and duplicate retry behavior.
- [x] With JavaScript embedded in the app, disconnect one instance, commit local work, terminate/relaunch it while disconnected, and reconnect. Assert retained data/queued work and eventual agreement with the backend.
- [x] Run affected Node/Dart and generated API regressions. Record exact commands, revision, toolchain, screenshots/assertions, and unsupported APIs in `packages/client-react-native/README.md` and the harness README.
- [x] Check every acceptance criterion in #100. Native linking or Node-only tests are insufficient. Once supported runtime evidence is complete, #31 can consume the SDK without implementing platform support itself.
