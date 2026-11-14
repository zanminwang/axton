# Named Actions over the real host

`bash integration/action-e2e/run.sh` compiles the current and retained-schema fixtures, typechecks the Node hosts (including removed-API negative checks), analyzes the three Dart hosts, and starts disposable PostgreSQL. The tests use generated typed clients, the native actor, real SQLite files, HTTP/WebSocket transport, and the generated backend.

The suite checks durable named Mutation optimism and `Call.wait`, exact retry after a lost accepted response, PostgreSQL serialization retries, immutable business results versus current Store authority, Query/Fetch store policies, client-expanded defaults and scalar codecs, explicit cross-file publication, canonical deletion, and aggregate acceptance/refusal. The Dart host exercises the same backend for millisecond DateTime precision and optional operands.

Each Store binds one Stream. Cross-viewer delivery uses independent files. The retired `onStore` hook, generated anonymous writes, enqueued Queries and multistream facades are fenced by runtime absence and compile-time negative cases. Backend handlers explicitly track newly published identities and invalidate changed/deleted identities.

The proxy observes or interrupts actual network exchanges; it does not install authority or advance a cursor. Read-only SQL checks inspect durable queue input and backend business rows. Teardown closes transport, drains entered backend transactions and then closes PostgreSQL resources; unexpected background errors fail the fixture.

The evolved fixture is compiled for generated compatibility only. This runner does not claim a retained-context replay proof; retained Model/Mutation history and Store upgrades are exercised by the [joined host gate](../sdk/README.md). Nor is this suite a literal replay of retired 0.3 Load/hook/multistream behavior.
