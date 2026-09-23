# React Native Support Specification (#100)

**Dependency:** [React Native support #100](https://github.com/zanminwang/axton/issues/100) blocks [To-do demo #31](https://github.com/zanminwang/axton/issues/31).

**Status:** Implemented and verified on an arm64 iOS simulator on 2026-09-15; see the harness README for evidence. See the [plan](../plans/2026-09-15-react-native-support.md) and [handoff](../handoffs/2026-09-15-react-native-support.md).

## Goal and scope

Enable an Expo React Native application written in TypeScript to use AXTON's generated client with the existing Rust/SQLite engine. Prove persistence and two-client synchronization before #31 consumes the SDK.

Keep it simple: reuse the engine, generated API and application-owned backend. The first acceptance target is an arm64 iOS simulator with embedded JavaScript. The harness lockfile selects Expo 57.0.22, React Native 0.86.3 and React 19.2.3.

To-do UI, User/Todo tables, avatars and replacement of `examples/rust-round-trip` belong to #31. Android, physical devices, background execution, WASM and standalone npm publication are outside this acceptance scope. Expo Go cannot load the custom native module.

## Architecture and interfaces

Generated TypeScript client → mobile SDK → Expo Swift module → Rust RuntimeHost → SQLite. HTTP and WebSocket use React Native's native APIs against the existing backend.

| Source | Responsibility |
| --- | --- |
| `bindings/mobile/` | Small C ABI carrier over RuntimeHost |
| `packages/client-react-native/native-module/` | Swift promises, persistent paths, library build and autolinking |
| `packages/client-react-native/` | Mobile entry point, explicit transaction scope and native transport |
| `packages/client-js/runtime.mts` | Existing orchestration shared by Node and mobile |
| `integration/bindings/client-react-native/` | Host-level adapter regression tests |
| `integration/platform/react-native/` | Diagnostic app, generated fixture and disposable backend |
| `integration/platform/run_react_native_ios_smoke.sh` | Two independent simulator installations and restart assertions |

The Expo module is named `AxtonNative`:

```ts
interface AxtonNativeModule {
  clientCall(request: string): Promise<string>;
  databasePath(name: string): Promise<string>;
}
```

The C ABI exports `char *axton_mobile_call(const char *input)` and `void axton_mobile_free(char *output)`. Swift executes on a serial background queue, copies the output, frees it exactly once, unwraps `{ok,result,error}` and returns serialized `result`. Invalid input and runtime failures reject promises; pointers never reach JavaScript.

The public helper is `databasePath(name = 'axton.sqlite'): Promise<string>`. It accepts a basename, creates Application Support as needed and resolves the same path across launches. Separate installations have separate client databases and identities.

Generate mobile code with the compiler's `--client-runtime` option; do not hand-edit generated files. Preserve the [binding contract](../../engineering/architecture/sdks/bindings.md), [typed client responsibilities](../../engineering/architecture/sdks/typed-api/client.md) and [engine guarantees](../../engineering/guarantees.md).

## Required behavior

- Generated reads, watches, writes, mutations and subscriptions work through the real mobile carrier.
- Records, client identity and queued mutations persist in SQLite and survive process termination and offline relaunch.
- Transactions serialize commands and isolate external reads. Thrown callbacks, caught native failures and outstanding operations prevent commit. Queued commands drain before rollback; retained transaction objects reject later calls.
- Mobile uses explicit transaction scopes without nested savepoints. Node keeps its existing AsyncLocalStorage/savepoint API.
- HTTP and WebSocket carry bearer authentication. Subscription acknowledgement precedes catch-up. Cancellation invalidates late work; the 64-page stream buffer recovers overflow from the durable cursor. Ordinary streamed updates require no periodic polling loop.
- Duplicate delivery and retry after a lost push response do not execute backend handlers twice.
- Mobile bundles must not load Node built-ins, N-API binaries or Node `ws`. Preserve existing Node/Dart APIs and generated types.

Native WebSocket errors lack structured HTTP status. HTTP 401 can use shared `refreshAuth`; socket-only authentication failures require application credential management. Document this limit rather than parsing error wording.

## Integration data and acceptance

The harness owns one application model: `Entry(id: String, text: String, note: String?)`, with `AddEntry` and `Edit` mutations. Engine-owned tables retain existing contracts. This fixture does not define the To-do product schema.

Two separate installations represent Alice and Bob. They exchange edits on `book:demo`. Alice's proxy drops the first successful push response to force receipt retry, then disconnects her. She creates a record and queues a dependent edit; her app is terminated and relaunched offline. Bob edits the original record. Alice reconnects.

Acceptance requires:

1. C ABI/path error tests and reviewed memory ownership, including invalid input, failed open and closed handles.
2. A complete Release simulator app that links and loads Expo/Swift/Rust with embedded JS.
3. Passing real native rollback, offline restart and two-client convergence assertions. Final results: distinct identities, Alice's stable identity, zero pending work, identical rows on both clients and PostgreSQL, one dropped response, one add handler execution and four edit executions.
4. Passing affected Node/Dart, compiler, generated API and host integration checks.
5. Actual revision, toolchain, commands, assertions and screenshots recorded in the harness README, followed by whole-branch review.

Host shims and successful static-library builds alone do not satisfy native acceptance. Keep #100 open as a dependency until these gates pass.
