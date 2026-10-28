# Protocol 5 server paths

## 1. Introduction and Goals

The Batch path commits each named Mutation independently and resumes a fixed request after a crash. Protocol 5 separates execution acknowledgment from locally committed settlement and uses finite authoritative delivery. [Adoption](../../protocol5-adoption.md) remains a separate release gate.

## 3. Context and Scope

`BackendOptions.protocol5` supplies known current/retained read schemas, projection generation and `authorizeStream(principal, stream, tx)`. The server derives the read context through core's protocol-5 materialization helper. Authentication identifies the principal. Mutation admission/replay and finite authority planning authorize under their Serializable transaction. Ordinary Query/Fetch validate the immutable Store binding and authorize without holding Store progress across arbitrary Handler/Loader awaits. Explicit Query tracking writes membership in its Serializable read transaction and may force a complete retry.

The SDK carries one validated immutable request through sequential `processBatchMember` native calls. Each call opens its own Serializable transaction. Rust validates the complete request, selects the trusted Mutation by name/version and normalizes its input. The caller's descriptor fingerprint is immutable intent bound by the Batch digest; it is not an equality gate against today's artifact. Compatible input widening or field reordering therefore does not refuse frozen work.

## 5. Building Block View

- [protocol_v05.rs](../../../../crates/server/src/protocol_v05.rs) validates carriers, assembles acknowledgements and adapts existing settlement/Loader machinery.
- [mutation_batch.rs](../../../../crates/server/src/mutation_batch.rs) owns Store admission, replay, per-member execution and outcome persistence.
- [delivery_plan.rs](../../../../crates/server/src/delivery_plan.rs) builds Bootstrap/repair cursor pages and owned materialization and carries Query/Fetch snapshots.
- [Server bridge](../../../../crates/protocols/src/server_bridge/mod.rs) defines strict `Protocol05Operation` requests, mirrored in [host-contract.mts](../../../../packages/backend/server/bindings/host-contract.mts).
- [persistence.mts](../../../../packages/backend/postgres/src/persistence.mts) dispatches those operations through the shared pg/Prisma/Drizzle driver; [migration.sql](../../../../packages/backend/postgres/migration.sql) installs five tables in a fresh framework namespace and refuses an existing legacy namespace unchanged.

## 6. Runtime View

A member locks its Store before checking immutable principal/Stream binding, Batch sequence, digest, count and progress. Only the current Batch or the last completed Batch can replay. A valid next Batch prunes the older result set in the same transaction. Saved members run neither handler nor preparation.

Existing Store ownership on delivery, materialization and completed reconnects is
inspected without locking Mutation progress. First claims and unfinished Bootstrap
initialization retain their guard. Progress writers use `FOR NO KEY UPDATE`, which
still serializes uploads while allowing references to the immutable Store
identity. See [read concurrency](read-concurrency.md) for the complete path audit
and real same-Store liveness tests.

Fresh work authorizes and performs business work in the same Serializable transaction. No namespace-wide publication lock is acquired. The business savepoint isolates an explicit typed refusal. A handler or Loader business refusal rolls back its effects, then saves rejection/progress outside that savepoint. Infrastructure, malformed Loader output or database failure aborts the outer transaction and leaves progress unchanged. The final saved member promotes current metadata to the last completed Batch.

Explicit tracking and invalidation use the retained discovery/recheck and canonical Stream locks. New/live pairs always have a non-null discoverable position. Each actual transaction reserves at most one cursor per affected Stream, reused by every preparation publication. Reservation state is checked against `pg_current_xact_id()` so a reused caller-owned connection cannot carry it across COMMIT. Existing live tracking retains its cursor; invalidation reaches existing holders without enrolling others.

Input targets are read in the same transaction snapshot after preparation. A tracked initiating-Stream target records its position and a call-owned null-cursor fallback. Other targets stay private. Returning an identity or snapshot never tracks it. Business rows, final positions, target evidence, result and progress commit together; subscriber wakes follow commit.

## 7. Finite authority delivery

Handshake authenticates the Store and Stream, runs Bootstrap preparation once, settles its tracking and saves the initial head in one Serializable transaction. Retries recover committed preparation; reconnect returns the current head.

Bootstrap preparation enrolls the complete initial scope. It shares track-only validation, deduplication and sticky declaration failures with Queries, but does not inherit a single read page's 1,000-pair/1-MiB enrollment bounds. Native materialization owns finite-plan capacity and transport paging.

Bootstrap selects the retained schema's marked Models and retains the first
handshake boundary S. Repair reads current positions above committed B/C, not
historical business versions. Loader preparation closes over required identities
before head capture; content and positions come from that same Serializable view.
Core groups same-cursor, unique-Model and cascade dependencies before canonical
loading. A page selects a prefix of complete components with a 500-change target;
a larger required component travels whole. Only selected records are canonically
loaded. Constraint-free queries seek a bounded cursor page with whole-cursor
ties and lookahead; constrained Models retain full metadata closure. Independent versions moved beyond the requested bound are deferred to
later Sync; required ahead companions do not fabricate covered progress.

All fragments of the selected page are returned together after the application
transaction commits. The header's `through` is the prefix actually covered,
which can be below the requested end. The next request starts at the client's
committed cursor and constructs a fresh view. No server response payload is
persisted, and no continuation reads an old plan. The fresh schema has no delivery
payload tables; reapplying its DDL drops those two retired transient tables while
retaining business/Store/Stream/outcome facts. Count capacity is 100,000 candidate
identities and response capacity is 256 MiB. The response lifetime bounds local
fragment staging, not a server cache. Old continuation carriers are rejected;
backend and client must be upgraded together.

The socket controller advances its connection-local offered cursor only at complete units and requests another fresh cursor page while below the observed head. Reconnect starts at its new handshake head; HTTP repairs missed coverage. Live headers name the backend's active materialization. A client retaining an older descriptor must treat a foreign-context live header as a repair hint and request HTTP authority under its own context, without installing the foreign payload or advancing its cursors.

Owned materialization binds saved settlement targets or a compatible schema owner. Explicit keys must already have StreamRecord evidence; selected schema Models must be Bootstrap-marked. It returns current authority or membership Remove, never enrollment or range coverage. Private receipt fallback remains the client's separate owned-settlement path.

Query/Fetch use the shared `ReadRequest`/`ReadResponse` carrier and existing retained descriptor normalization, Loader state normalization and Query snapshot assembly. Records carry explicit null cursors. Query exposes explicit tracking through its authenticated Stream context, settled through Publication05 in the same read transaction. It exposes no invalidate or business-change declaration handles; storing a snapshot never implies enrollment. Repeated live tracking retains its position; a new pair receives a positive Stream position. Business refusal rolls back its read savepoint. Infrastructure failures abort the outer transaction.

## 10. Quality Requirements

[protocol-v05-batch.test.mjs](../../../../integration/persistence/server/protocol-v05-batch.test.mjs) inspects actual PostgreSQL rows after partial execution, process exit, refusal, infrastructure failure, duplicate replay, publication races and overflow. [protocol-v05-delivery.test.mjs](../../../../integration/persistence/server/protocol-v05-delivery.test.mjs) covers frozen interleavings, expiry cleanup, atomic unique transfers, capacity rollback, ordinary reads, socket/reconnect and 10k/100k capacity measurements. Measured costs do not establish a production latency promise; end-to-end native client installation and settlement remain adoption gates.
