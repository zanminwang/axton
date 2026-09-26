# `@axton/native`

Reusable iOS Expo module for AXTON's Rust client carrier.

The Expo module is named `AxtonNative` and exposes:

```ts
interface AxtonNativeModule {
  runtimeOpen(request: string): string;
  runtimeSubmit(runtimeId: string, message: string): void;
  runtimeDrain(runtimeId: string): string;
  runtimeDetach(runtimeId: string): void;
  // event "axtonWake": { runtimeId: string }
  clientCall(request: string): Promise<string>;
  databasePath(name: string): Promise<string>;
}
```

The `runtime*` functions carry the Rust-owned client runtime of
[#134](https://github.com/zanminwang/axton/issues/134) for the shared JS
Bridge. They are synchronous and never wait for a task: `runtimeOpen` starts a
runtime on its own thread and answers its id, `runtimeSubmit` admits one
envelope (it throws `client_closed` once the runtime is gone), `runtimeDrain`
answers the published events as JSON array text, and `runtimeDetach` stops the
runtime's wakes for good. The runtime's thread signals new events through the
`axtonWake` event, posted on the main queue; the package's `index.ts`
registers one listener and routes each wake to its runtime's Bridge. The
module detaches every runtime it still holds when it is destroyed, so no wake
can reach a freed module.

`clientCall` is the former synchronous command host, kept while the SDKs move
to the runtime; it runs on a dedicated serial queue, resolves with the
serialized, unwrapped `RuntimeHost` result and rejects carrier or runtime
errors. `databasePath` accepts one basename and returns a stable path in the
app's Application Support directory, creating that directory when needed.

The pod's prepare step builds the Apple Silicon simulator slice. Run
`bash scripts/build-ios.sh simulator` directly to rebuild it. The device slice
is configured with `bash scripts/build-ios.sh device`, but remains unverified
until it is built and exercised on a device.
