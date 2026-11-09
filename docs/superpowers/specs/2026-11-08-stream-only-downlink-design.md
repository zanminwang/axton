# Deliver authority only through the Stream

Issue: Capso CAP-879 (no AXTON issue yet). Status: approved (2026-11-08). Slug: `stream-only-downlink`.

A Store receives authoritative records only from its own Stream, by `/sync/pull` ranges or the live socket. `/sync/materialize`, the point fetch by key, is removed. Settlement waits for the Stream instead of fetching, and a materialization change drains the queue, clears server-derived rows and runs Bootstrap again. Done means no client sends a materialize request, every Mutation still settles, rollover keeps queued Mutations and device-only rows, and Capso's 120-Reply restore test passes in CI.

## Contents

1. [Problem and evidence](#1-problem-and-evidence)
2. [Design](#2-design)
3. [Changes](#3-changes)
4. [Decisions](#4-decisions)
5. [Verification](#5-verification)
6. [Out of scope](#6-out-of-scope)
7. [Open questions](#7-open-questions)

## 1. Problem and evidence

Capso's `entry_replies_test.dart` pushes 60 Replies per device and times out in CI. Measured on the Capso test backend:

- After each Batch acknowledgement, the client sends one `/sync/materialize` per accepted Mutation whose Stream target lacks authority (`crates/client/src/sync05/downlink.rs:586`). `owned()` dedupes per owner and never caps concurrency (`downlink.rs:672`). 432 settlement requests in one run, one key each, peak 108 in flight per user.
- Each request runs the Reply Loader hook. Capso's hook re-tracks the Reply into the viewer's Stream. `settle_changes` locks that Stream row even though every pair is already tracked and nothing is written (`crates/server/src/settlement.rs:143-146`, `publish=false` at `:182`). Under Serializable isolation the waiters fail with 40001 instead of queueing.
- 109 transactions exhausted 8 retries and returned 500. The client resent them, up to 73 transactions shared a 20-connection pool, and the CI runner hit P2028 and stalled.
- With AXTON transactions serialized by a debug gate: 0 conflicts, 0 failures, 19 s instead of 30 s.

The original design settled a Mutation once normal Sync covered its `syncCursor` ([uplink-downlink-design.md:129](2026-10-07-uplink-downlink-design.md)). The [review](2026-10-07-internal-refactor-review.md) recommended a materialization for "an explicitly tracked but locally missing target". The [spec](2026-10-07-internal-refactor-spec.md) (§5, line 202) made it unconditional, and the implementation fires it at acknowledgement time without waiting for coverage. [Guarantees](../../engineering/guarantees.md) already state "Only Stream delivery establishes authority"; materialize is the one authority path that is not a Stream range.

Two real needs hide behind it, and this design covers both without it:

1. **Historical targets the Stream will not deliver to this Store.** Bootstrap selects only `@@bootstrap` Models over `(B, S]`, and normal Sync starts after `S` (`delivery_plan.rs:167-179`, `settlement05.rs:613-621`). A Mutation input in an unmarked Model whose Stream position is at or below `S` never arrives by pull or live.
2. **Materialization rollover.** After a compatible schema change, held rows must be re-proven in the new read contract. Today `pending_schema05` sends held keys to materialize (`store05.rs:255-304`, `downlink.rs:568-579`).

## 2. Design

### 2.1 Delivery paths after this change

| Path | Carries | Establishes authority |
| --- | --- | --- |
| `/sync/pull` (Bootstrap and Sync) | Stream ranges | yes |
| live socket | Stream ranges | yes |
| Query and Fetch results | cursorless snapshots, installed as cache | no (unchanged) |
| Batch acknowledgement | `syncCursor`, targets, fallback and private snapshots | no; finalizes owned state for Private targets (unchanged) and for undeliverable historical Stream targets (§2.2 rule 3) |

`/sync/materialize`, `MaterializationRequest`, `MaterializationResponse`, `MaterializationOwner`, `DeliveryHeader.owner`, `DeliveryPurpose::Settlement`, `DeliveryPurpose::Schema`, host `readResults` and `deliveryCandidates.keys` are deleted. The server's `materialize()` Loader step in `delivery_plan.rs:256` is shared with pull and stays; it is not the route.

### 2.2 Settlement

The per-target evidence rule stays (`target_disposition`, `settlement_ready`; guarantees A3, A4). Coverage alone is not enough: compaction can move a target from `t` to `t' > syncCursor`, so `C >= syncCursor` can hold while the target is still in a later unit.

What changes is how missing evidence is obtained. The client never fetches; it guarantees the Stream will bring the evidence, and settles from the receipt where the Stream cannot.

1. **Push waits for the handshake.** `freeze05` does not freeze a Batch until `start_cursor` is set. A Mutation pushed through the normal path then executes after the handshake head read, so `syncCursor >= S`.
2. **The receipt raises the delivery head.** On `Acknowledge`, `delivery_head = max(delivery_head, max syncCursor)`. `schedule()` then pulls `(C, syncCursor]` when live has not delivered it. Today only handshake, socket and delta responses raise it, so a Store with a silent socket waits indefinitely.
3. **Targets the Stream cannot deliver settle from the receipt.** A Stream target with cursor `t <= S` in a Model that is not `@@bootstrap` in the Store's active materialization (read from `retained_schema05(enabled_descriptor)`, not the app schema, so a drain under the previous materialization uses the previous flags) is never delivered to this Store. Its disposition becomes `FinalizeOwned` using the receipt's `fallback` snapshot, the same rule a Private target uses today: finalize owned state as cache, no evidence, no tracking. Without `current` protection, a later Query or Fetch snapshot may replace that row, older content included; the Store holds no Stream content for it, so no guarantee covers it. All other Stream targets wait for evidence at or after `t`, or a later Remove. `@@bootstrap` targets at or below `S` arrive when `B` passes them.

`target_disposition` gains the inputs `start_cursor` and the target Model's bootstrap flag. `pending_settlement05().missing_keys` is deleted.

### 2.3 Materialization rollover: drain, clear, Bootstrap

A Store whose `materialization` differs from `desired_materialization` becomes a fresh device once its frozen work is settled. A record is **server-derived** when its `axton_authority` evidence has a non-empty `history` or a `membership` (`crates/core/src/authority.rs:31-35`). Direct writes, Query and Fetch cache installs and receipt-finalized rows leave empty evidence (`settlement05.rs:14-26`, `:603-604`, `:245-249`); the data cannot tell them from device-only rows, so they are kept.

1. **Hold.** Stored reads park with `read.schema_pending` (unchanged). `freeze05` freezes nothing new. Mutation calls still enqueue and apply optimistically; they wait unfrozen.
2. **Drain.** Frozen Batches (`batch_id IS NOT NULL AND reconciled = 0`) push in their frozen context and settle by §2.2 under the previous active materialization. Rejections, including `mutation_version_unsupported`, are ordinary rejections. Unfrozen Mutations, including ones blocked by prerequisites or dependencies, are not drained.
3. **Clear.** One SQLite transaction:
   - deletes every server-derived row from its Model table, its `axton_before_*` row and its local layers, except keys that unfrozen Mutations hold dirty (`queue.rs:341-348`); for those it deletes only the `axton_authority` row, so the next delivery admits as `Newer` and `stage_one` re-bases them before pending operations replay (`authority.rs:113-128`);
   - deletes delivery staging (`axton_delivery_progress`, `axton_delivery_key`);
   - sets `materialization = desired_materialization`, `enabled_descriptor = schema_descriptor`, and `start_cursor`, `bootstrap_cursor`, `cursor` to null.
   Rows with empty evidence, the Store ID, the Mutation and Batch counters and reconciled queue rows stay, so Batch numbering never repeats.
4. **Bootstrap.** The downlink handshakes again. When the rollover was discovered by a handshake and no Batch was frozen, the clear commits together with `initialize_stream05` of that handshake's head instead. The server does not run `handleBootstrap05` again for the same Store ID (`delivery_plan.rs:76-104`); its tracked records are already Stream members. Bootstrap pulls the `@@bootstrap` Models in the new context, freezing resumes, and parked reads release at `Initialize`, as on a fresh device.

The guarantee: queued Mutations are never dropped, rows the device wrote or cached survive (in the read contract they were written in), and every rollover re-proves the `@@bootstrap` Models. Server-derived rows of unmarked Models are cleared and return through their next Stream change or a Query. A rollover needs the network; offline, reads stay parked.

**Release rule.** Every change of materialization (a Model contract or the projection generation) ships with `protocol5.materializations[<previous id>]` on the backend (`crates/server/src/protocol_v05.rs:229-237`, admitted by `materialization.rs:14-52`) until clients have drained. Without it a Store with a frozen Batch gets 409 `context_mismatch` and never finishes the drain.

### 2.4 Server materialization change

`projection_generation` is backend configuration (`crates/server/src/protocol_v05.rs:229-240`). The client takes its value from app code at open (`store05.rs:110-121`) and never learns that the backend moved. If the backend drops the old context, every request returns 409 `context_mismatch`, and the downlink retries it forever (`downlink.rs:246-252`).

The backend becomes the only source of the generation.

- `HandshakeResponse` and the socket handshake acknowledgement gain `projectionGeneration` (`delivery_plan.rs:107-131`, `live.rs:222-238`).
- The SDK `projectionGeneration` connection option is removed (`client-js/api/runtime.mts:348`, `dart/lib/src/api/client.dart:148`, `bindings/common/src/actor.rs:408-417`). Open computes `desired_materialization` from the app schema and the generation stored on the schema descriptor (`axton_descriptor.projection_generation` of `schema_descriptor`), so a reopen never undoes a server-driven change. A fresh file starts at `"1"`.
- When a handshake reports another generation, one runtime SQLite transaction writes the new `axton_descriptor` row, `schema_descriptor` and `desired_materialization`. §2.3 then runs; a fresh file has nothing to drain.

## 3. Changes

Surfaces in landing order. The plan carries the full per-test removal list. Everything this change leaves unused is deleted in the same release: code, types, SQL, host operations, tests, fixtures and docs. No dead code, deprecated aliases or compatibility shims remain (user, 2026-11-08).

**Protocol (`crates/protocols`)**
- `src/sync.rs`: delete `MaterializationOwner`, `MaterializationRequest`, `MaterializationResponse`, `validate_materialization`, `DeliveryPurpose::Settlement` and `::Schema`, `DeliveryHeader.owner`, `admit_owned_context` (header admission becomes plain `context.admit`); add `HandshakeResponse.projection_generation`; `PROTOCOL` becomes 6.
- `src/sync/delivery.rs`: delete `freeze_materialization` and the owner arm of `freeze`/`DeliveryHeader::validate`; extend `target_disposition` per §2.2 rule 3.
- `src/client_bridge/mod.rs`: delete `HttpRoute::Materialize`.
- `src/server_bridge/mod.rs`: delete `Protocol05Operation::ReadResults` and `DeliveryCandidates.keys`.
- `fixtures/protocol/0.5.json`: delete materialize entries, drop `owner` from headers, regenerate digests.

**Server (`crates/server`, bindings, TS backend)**
- `crates/server/src/delivery_plan.rs`: delete `process_materialization05` and the `keys` parameter of `prepared_candidates`; `handshake05` returns the projection generation.
- `crates/server/src/live.rs`: drop the owner check; the socket handshake acknowledgement carries the projection generation.
- `bindings/node/src/server.rs`: delete `process_materialization05`.
- `packages/backend/server/connection/http.mts`, `api/backend.mts`, `bindings/native.mts`, `bindings/host-contract.mts`: delete the route, dispatch, native entry and `readResults`.
- `packages/backend/postgres/src/persistence.mts`, `sql.mts`: delete `readResults`, `V05_RESULTS_READ` and the keys branch of `deliveryCandidates`.

**Client (`crates/client`)**
- `sync05/downlink.rs`: delete `Flight::Owned`, `owned()`, settlement and schema request building; raise `delivery_head` on acknowledgement; run the rollover drain, then clear and re-handshake.
- `sync05/delivery_queue.rs`: delete `owner`, `owner_keys` and `receive_owned`.
- `sync05/delta_applier.rs`: delete the owned-plan path and the Schema-owner enable.
- `mutation_queue.rs`: `freeze05` waits for `start_cursor` and freezes nothing while a rollover is pending.
- `settlement05.rs`: delete `missing_keys`; pass `start_cursor` and bootstrap flags to `target_disposition`.
- `store05.rs`, `sync05/reset.rs`: `PendingSchema05` drops `authority_keys` and `bootstrap_models`; delete `enable_schema05` and `stage_materialization05`; add the §2.3 clear, sharing table walking with `reset_store05`; handshake handling applies §2.4.

**SDKs**
- `packages/frontend/client-js/bindings/transport.mts`, `connection.mts`; `packages/frontend/dart/lib/src/bindings/live.dart`, `connection.dart`: delete the materialize route.
- `packages/frontend/client-js/api/runtime.mts`, `packages/frontend/dart/lib/src/api/client.dart`, `bindings/common/src/actor.rs`: remove the `projectionGeneration` option.

**Docs**
- `docs/engineering/guarantees.md` (delete the receipt-owned Materialize paragraph; state §2.2 rule 3), `architecture/protocols/sync.md`, `architecture/server/protocol5.md`, `architecture/client/**`, and the website pages that name the route, `HttpRoute::Materialize` or receipt-owned materialization.

## 4. Decisions

- **Delete materialize rather than cap its concurrency.** Rejected: a client-side cap of N in-flight settlement requests. It fixes CAP-879 but keeps a second authority path whose only users are the two cases in §1, both of which the Stream or the receipt can serve.
- **Keep per-target evidence; do not settle on `C >= syncCursor` alone.** Rejected: the original coverage-only rule. Compaction can place a moved target beyond the unit that covers `syncCursor`; settling there drops the local layer over an older base.
- **Finalize undeliverable historical targets from the receipt.** Rejected: delivering every Stream member in Bootstrap so no target is undeliverable. That turns `@@bootstrap` into a no-op and makes every new device download the full Stream history. Cost: the finalized row is cache, so a later, older Query snapshot can replace it.
- **Roll over by drain, clear and Bootstrap.** Rejected: re-pulling `(0, C]` into a staged lane while keeping local state, which keeps direct writes but needs a second staging path beside normal coverage. Rejected: clearing without draining, which leaves pending updates replaying over missing rows and marks them diverged for good (`mutate.rs:254-256`). Upgrades are rare and Bootstrap is seconds, so a fresh-device rollover is enough (user, 2026-11-08).
- **Server-derived means Stream evidence (`history` or `membership`).** Rejected: classifying Models as device-only on the client, since Loaders exist only in server config. Rejected: "has an `axton_authority` row", which direct writes and cache installs also create.
- **Drain only frozen Batches and keep dirty keys.** Rejected: draining the whole queue, which stalls on a prerequisite-blocked Mutation. Rejected: parking Mutation calls, which cannot work inside an open local transaction.
- **Rollover Bootstraps only `@@bootstrap` Models.** Rejected: re-pulling every Model, which downloads unmarked history the Store never held. Unmarked records come back through Queries, as on a fresh device (user, 2026-11-08).
- **Push waits for the handshake.** Rejected: settling Mutations with `syncCursor <= S` specially. Gating the push is one condition, and `start_cursor` commits only after a handshake response, so no pushed Mutation executes before `S`.
- **The server is the only source of the projection generation.** Rejected: leaving it app-supplied, which today strands clients in a 409 loop after a backend change. Rejected: accepting both, since a reopen with the app value would undo the server's.
- **No compatibility window; breaking release 0.6.0.** The server drops `/sync/materialize` in the same release, and clients and server upgrade together. Rejected: keeping the route for one release. There are no external installs to protect (user, 2026-11-08).
- **Wire protocol 6, SQLite format 5 kept.** The wire constant moves to 6 so a 0.5.x peer gets a clean `unsupported_protocol` instead of digest or route errors. Existing SQLite files stay valid; open clears transient delivery staging (`axton_delivery_progress`, `axton_delivery_key`), which may hold owned plans. Rejected: requiring fresh files, which would discard local work for no storage change.

Design review (Fable 5.1, 2026-11-08) changed: §2.4 makes the backend the only generation source so a reopen cannot undo it; §2.2 drops an unreachable replay case and names the cache cost of rule 3; §2.1 lists what the receipt now finalizes. Its findings on the range-rollover lane (admission, enable condition, durable lane progress, paging cost) were superseded when rollover became drain, clear and Bootstrap. The second pass changed: the server-derived classifier, drain scope and dirty-key handling, the single-transaction clear for a handshake-discovered rollover, `enabled_descriptor` in the clear, the bootstrap-flag source for rule 3, the release rule, and the rollover test fixture. Implementation (2026-11-08) changed §2.4 to read the generation from `schema_descriptor` instead of the enabled descriptor: between a handshake that reports a new generation and the clear, a reopen would otherwise restore the old desired materialization and could freeze a new Batch in the old context.

## 5. Verification

| Check | Layer |
| --- | --- |
| No client effect ever targets `/sync/materialize`; the route returns 404 | `bindings/common/tests/protocol05_runtime.rs`, `integration/bindings/client-js/live.test.mjs` |
| A Store with no socket settles a Mutation after acknowledgement through a pull of `(C, syncCursor]` | `crates/sqlite/tests/protocol05_downlink.rs` |
| A target moved by compaction past `syncCursor` keeps its local layer until the newer position lands | `crates/sqlite/tests/protocol05_settlement.rs` |
| An unmarked target at or below `S` finalizes from the receipt fallback with no evidence; a marked one waits for `B` | `crates/sqlite/tests/protocol05_settlement.rs` (replaces `historical_receipt_target_settles_below_prefix_while_public_bootstrap_remains_incomplete`) |
| No Batch freezes before `start_cursor` is set | `crates/sqlite/tests/protocol05_queue.rs` |
| Rollover drains frozen Batches, then clears exactly the server-derived rows. The fixture holds a direct-written device-only row, a Query cache row, a receipt-finalized row, a Stream row of a marked and of an unmarked Model, and a dirty row of an unfrozen prerequisite-blocked Mutation; afterwards only the two Stream rows are gone, the dirty row re-bases on delivery, and the Store ID and counters are unchanged | `crates/sqlite/tests/protocol05_queue.rs` |
| A fresh file with queued Mutations against a backend at generation `"2"` rolls over at the first handshake and pushes them in the new context | `integration/v05-sdk/client.mjs` (replaces the A14 owned-transfer cases) |
| A backend projection change triggers rollover instead of a 409 loop | `integration/v05-sdk/client.mjs` |
| 60 concurrent Mutations from one Store settle with no request beyond push and pull, and serialization retries stay below 10 | new case in `integration/v05-sdk/client.mjs` against PostgreSQL |
| Capso `entry_replies_test.dart` passes five consecutive CI runs after upgrading AXTON | Capso mobile integration job |

## 6. Out of scope

- Capso's Reply `prepareForViewer` re-track and the unconditional `lock_streams` on already-tracked pairs. Both stay possible sources of same-Stream conflicts; without materialize they no longer run once per acknowledged Mutation. Tracked separately.
- How long a backend retains old Mutation versions and materializations. §2.3's release rule covers the previous materialization during drain; an unretained frozen Batch context still fails the whole Batch with 409.
- Query and Fetch snapshot installation.
- A public reset API change; `resetStore` keeps its behavior.

## 7. Open questions

None.
