# Guarantees

The behavioral contract of the Rust sync core. These are requirements, not a claim that every case is implemented or tested. [Coverage review](testing/review.md) records known gaps and decisions still needed; [component documentation](architecture.md) owns API, encoding, schema and adapter rules.

Simulation exercises these behaviors across clients and message sequences. Real-database tests must also verify claims that depend on persistence or transaction semantics. IDs remain stable so scenarios can refer to them. Existing references to a mutation describe the retained queue and replay machinery; generated application APIs expose Actions for backend work and local-only Model CRUD for local writes.

## Failure isolation

Applications use individual operations as they would ordinary API calls. This applies to mutations and loader reads: a failure attributable to one operation must be reported for that operation, without rejecting or blocking unrelated work merely because it shares a batch, page or channel. Explicit application transactions and declared dependencies still define shared outcomes. Transport or database transaction failure may require retrying delivery; it must not be reported as business rejection of every operation in that delivery.

## L. Local writes

| ID | Required behavior |
| --- | --- |
| L1 | Reads show the authoritative base with pending local edits replayed in order. A transaction sees its own writes; other readers see them after commit. |
| L2 | Committed records, queued mutations and rejections survive database reopen. |
| L3 | A failed transaction rolls back its changes. A nested savepoint can roll back its own scope without discarding the outer transaction. |
| L4 | Direct writes never enter the push queue. Companion edits follow their mutation's acceptance or rejection. Direct edits to an authoritative record survive rejection of a pending mutation, but later server authority may replace them. A record whose create is still pending has no authoritative base: if the create is rejected the record goes, direct edits included. A page the client has already applied does not undo a direct edit. |
| L5 | Local deletion applies the cascades declared by the schema, with the same rollback scope as the initiating operation. |

## P. Push

| ID | Required behavior |
| --- | --- |
| P1 | Retrying the same frozen batch with the same client ID and sequence returns its durable receipt without executing its handlers again. This relies on persisted client identity and sequence, and atomic server receipt storage. |
| P2 | The server accepts new batches in contiguous sequence order, returns cached receipts for retries, and refuses gaps or overlaps without executing them. |
| P3 | Unready prerequisites block their mutations. Lifecycle dependents wait for predecessor acceptance; sequence dependents may follow their predecessor in the same batch. Independent ready work can proceed. |
| P4 | Frozen request bytes remain unchanged across retries, restart and supported schema reconciliation. |
| P5 | Explicit rejection removes the mutation's optimism, rejects its lifecycle dependents, and retains a readable rejection until dismissed. Other pending edits replay on the remaining base. |
| P6 | A failure attributable to one mutation rolls back its savepoint, including its business changes, stamp allocations, loader readback and publications, without rolling back unrelated successful mutations. If infrastructure failure makes the enclosing transaction unusable, the delivery rolls back atomically with its receipt and can be retried; this is not rejection of all mutations. |
| P7 | A mutation-level rejection, including an unsupported mutation version, is recorded for that mutation in the receipt and retained as a client rejection (P5). It does not reject unrelated mutations merely because they share a batch. Declared lifecycle dependencies still apply; independent valid mutations can proceed. |

P6 and P7 are implemented: handler rejections, loader refusals, an unsupported mutation version (`mutation_version_unsupported`), a handler failure (`handler.failed`), a loader failure (`loader.failed`) and an undeclared or unretained changed model (`model_version_unsupported`) each roll back only that mutation's savepoint and become its rejection. The only failures that still take down the whole delivery are the request envelope and authentication (`400`/`401`), client identity and order (`403 client.owner_mismatch`, `409 gap`/`overlap`), and infrastructure — a failed `rollback` or a persistence fault in `claim`, `saveReceipt`, `advanceStamp`, `ensureStamp`, `publish` or `scan`. See [Server / Push](architecture/server/engine/push.md#9-architecture-decisions) and [#95](https://github.com/zanminwang/axton/issues/95).

## Q. Action outcomes

| ID | Required behavior |
| --- | --- |
| Q1 | A durable Action commits canonical intent and inferred local Model operations together. It may have no Model operations. Frozen intent bytes and call ID remain unchanged across retries and reopen. |
| Q2 | Each committed call ID has one immutable backend outcome. Claim, business writes, Loader result snapshots, stamps, publications and saved response share the application transaction. A replay returns the stored outcome without re-running the handler or Loader; no backend TTL or automatic pruning removes it. |
| Q3 | A queued Action handle reports local acceptance before backend completion. Its `wait()` resolves with either its typed result or an `ActionError`; an initial local failure rejects before the handle exists. Client result objects live in memory, while queue/completion state persists. |
| Q4 | Direct Actions use a finite request/response timeout, skip durable queueing and automatic optimism, and do not drain unrelated queued work. The direct response applies committed authority using the same stamp and pending-replay rules as a receipt. A timeout may leave execution unknown. |
| Q5 | A Model output is the versioned Loader snapshot for its invocation, not the batch-final record authority or the current optimistic local view. Explicit Model outputs use handler-returned identity objects. Nullable outputs, lists and void preserve their declared shapes. |
| Q6 | Diagnostic callback exceptions after commit cannot replace the Action outcome, re-execute the handler or become transport errors; SDK runtimes report them through their uncaught-error channel. |
| Q7 | A call's `store` option controls only the additional authority its explicit Model outputs contribute. Authority required by mutation inputs and handler-reported changes is always kept, results are unchanged, and the policy is persisted with the durable call and part of its call identity. |

See [Action protocol](architecture/protocol/actions.md), [server execution](architecture/server/engine/README.md) and [typed client](architecture/sdks/typed-api/client.md). Tool behavior remains [#143](https://github.com/zanminwang/axton/issues/143).

## A. Authority and settlement

Channel cursors order delivery within a subscription. Record stamps order authoritative content across every path that delivers it: a push receipt and a channel page carry the same kind of authority, compared the same way.

| ID | Required behavior |
| --- | --- |
| A1 | Delivered server values replace settled optimism; later pending edits replay over the authoritative base. |
| A2 | Within a subscription, each channel's cursor never decreases and moves only by whole pages: a page is applied as one unit and then every channel it names moves to that page's end. A channel whose range is already covered contributes nothing; a page whose range starts beyond a channel's local cursor is a gap and is not applied at all until a pull fills it, and a live frame with a gap is held, never dropped. A page answering a pull issued under an earlier subscription of the channel is stale, not a gap: it is dropped and the cursor stays where the resubscribe put it. |
| A3 | A successful push completes from its receipt alone. The receipt carries the authoritative content and stamp of every record the accepted operations targeted, read back by the framework in the handler's transaction; the client applies that authority, removes the completed operations and replays what remains in one local transaction. No channel is awaited, before or after, and a subscription is never required to complete a mutation. |
| A4 | Receipt authority is applied by the same stamp rule as page authority (D2): a newer stamp lands, an older one is ignored, an equal one with equal content rewrites nothing. Whichever arrives first, the receipt and the page for the same change leave the same state, and neither is skipped because the other already landed: a receipt whose authority the page already delivered still completes the batch, and a page whose authority the receipt already delivered still advances the cursor. |
| A5 | A receipt that cannot be applied, because it answers another client or batch, omits the authority of an accepted operation, or carries authority the client cannot decode, changes nothing: the frozen batch stays for retry. A receipt for a batch already completed changes nothing either. Completion is durable and survives restart without re-sending or re-applying. |

A3–A5 replace the earlier checkpoint contract, under which accepted optimism waited for channel positions named by the receipt and settled in batch order ([#52](https://github.com/zanminwang/axton/issues/52), superseded by [#55](https://github.com/zanminwang/axton/issues/55)). Only one batch is in flight at a time, so completion order is send order without a separate rule. See [Settlement](architecture/client/engine/settlement.md).

## D. Distribution

Convergence assumes valid backend records, correct publication, and eventual delivery. D7 defines isolation when an individual load fails. Convergence means authoritative content agrees after pending work settles; direct-only local data is outside that comparison.

| ID | Required behavior |
| --- | --- |
| D1 | Clients subscribed to the same channel converge on its authoritative records once changes stop and receipts and pages have been delivered. |
| D2 | A newer record stamp replaces authority; older content cannot regress it. Equal stamps with equal content are idempotent; conflicting equal-stamp content does not replace the stored value. |
| D3 | A record's stamp advances once per successful change, whether or not the change is published, and never merely to distribute a version: publishing allocates channel cursors, not stamps. Every channel a version is published to carries that one stamp. The first publication of a record with no stamp establishes one; later publications of an unchanged record reuse it. Channel cursors advance independently of record stamps. |
| D4 | The same record identity, model read version and stamp describe the same authoritative content on every delivery path: a receipt, a pull page and the live stream. Loaders name no channel; channels select which records are delivered, never alternate contents. A change to a record that several channels provide reaches each of them at the same stamp, and a delayed page from any of them cannot regress a newer version. |
| D5 | A newer deletion removes the content; an older deletion cannot erase newer content. The stamp of a deleted record is retained as evidence, so older content delivered later cannot resurrect it. A parent's authoritative deletion cascades to its declared descendants locally without rewriting their stamp evidence. |
| D6 | A channel is a delivery path, not an owner of local records. Unsubscribing stops that channel's delivery and resets its cursor; it removes no content, stamp, before image or pending operation. Retained data remains readable, is not promised to stay fresh without an update source, and is still updated by any other path that delivers a newer version. A loader's null record is a deletion. |
| D7 | A failure attributable to a loader read is reported to the application for the affected record. A loader may throw or refuse; no durable loader-failure queue is required. Unrelated reads proceed even within the same page or channel, and the cursor still advances. Failed reads do not erase local data, become deletions, change the record's stamp or count as synchronized authority; the record is corrected the next time it is delivered. |
| D8 | Nothing a delivery cannot apply is silent. A read failure, a change the client's schema refuses, an equal-stamp conflict and a pending edit that no longer replays over new authority are each reported to the application; none of them changes local content or stamps except the divergence, which shows the authoritative base while the edit stays queued and is still sent. |

D7 and D8 are implemented for pages and receipts ([#95](https://github.com/zanminwang/axton/issues/95), [#51](https://github.com/zanminwang/axton/issues/51), [#122](https://github.com/zanminwang/axton/issues/122)). A model the client did not declare still fails the whole pull. Inside a push a loader refusal or failure is that mutation's rejection (P6). See [Server / Pull](architecture/server/engine/pull.md#9-architecture-decisions) and [Client / Pull](architecture/client/engine/pull.md).

## R. Resilience

| ID | Required behavior |
| --- | --- |
| R1 | Local reads and writes continue offline. Queued work resumes and converges when connectivity, subscriptions and backend processing recover. No delivery-time bound is promised. |
| R2 | Dropped, duplicated, delayed and reordered messages preserve the safety properties above. Eventual convergence requires delivery to resume; permanent message loss cannot provide progress. |
| R3 | After a process interruption, durable committed sync state can be reopened and resumed without losing committed work. This is a recovery requirement; close/reopen tests alone do not establish arbitrary crash-boundary coverage. |
| R4 | A stale client writer cannot overwrite state committed by a newer writer generation. |

See [Testing](testing.md) for responsibility and code maps, and [Coverage review](testing/review.md) for the evidence still needed.
