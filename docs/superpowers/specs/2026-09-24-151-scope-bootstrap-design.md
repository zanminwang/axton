# Whole-Scope bootstrap with independent durable progress

Status: design draft for review; no implementation is claimed. Issue: [#151](https://github.com/zanminwang/axton/issues/151). Requires [persistent subscriptions](2026-09-24-150-persistent-subscriptions-design.md).

## 1. Goal and approved product semantics

Applications can prepare an entire Scope's initially published data, await local readiness, or let the same durable task run while the UI continues. The existing Engine owns scheduling, transport, authority application and restart recovery. Bootstrap never rewinds or advances the normal subscription cursor.

The user confirmed publication-based coverage for this milestone. Bootstrap enumerates identities published to the Scope and resolves them through the current owner's canonical Model Loaders. It does not infer current membership from business relationships, discover unpublished records, implement move-out rules, delete local content on unsubscribe, or maintain arbitrary query results. A Loader null is existing authoritative absence; an authorization/read error is not absence.

Coverage is not a transactionally frozen, whole-Scope point-in-time snapshot. Bootstrap and ongoing delivery jointly process the initial publication coverage, subject to the existing reported read-failure contract; newer authority may also arrive. Later synchronization continues normally. This distinction must appear in public documentation.

## 2. Public API proposal

```ts
const subscription = await client.scopes.subscribe("project:123");
await subscription.bootstrap();

// Same task; no separate background implementation.
subscription.bootstrap().catch(showLoadError);
```

Add `bootstrap(): Promise<void>` in TypeScript and `Future<void> bootstrap()` in Dart. Both methods eagerly submit registration through the SDK's existing serialized local command path, regardless of whether the returned Promise/Future is awaited. Rust exposes an explicit synchronous registration command and event/status-based observation; do not rely on polling a dropped lazy Rust future to start work.

Resolve only after the completion transaction commits. Calls during one active run share its work; calls after valid completion resolve locally, even offline. A completed call does not promise current online freshness. A new call after a terminal run failure explicitly retries the saved page/run. Background callers handle the returned rejection; status observers also receive the stored failure. Runtime observer errors never retry or undo committed work.

Extend Subscription status with:

```ts
type BootstrapStatus = Readonly<{
  phase: "not-requested" | "waiting-for-initialization" | "loading" |
         "catching-up" | "complete" | "failed";
  error: null | Readonly<{code: string; message: string}>;
}>;
// subscription.status.bootstrap: BootstrapStatus
```

Only the first active call registers work. Waiting for connectivity is not failure and has no implicit overall timeout. Explicit unsubscribe rejects waiters with `subscription.closed` and removes that epoch's load state. Client close rejects this process's waiters with `client_closed` but preserves the task; a reopened client resumes it even without a new bootstrap call. No independent persistent-task cancel or forced-refresh API is introduced in this milestone.

## 3. Bootstrap and subscription jointly cover delivery

The user clarified that Bootstrap is bound to a persistent Subscription. It fills the historical interval before that subscription's origin; the subscription supplies subsequent publications. Bootstrap need not independently enumerate every identity. This replaces the earlier draft proposal for immutable `first_cursor` metadata and identity-key pagination; neither is required.

Let S be the persisted `starting_cursor`, B the separate bootstrap progress (initially zero), and L the ongoing subscription cursor. Bootstrap scans `(B, S]`, while normal delivery continues after S. A record republished from cursor 40 to 120 when S=100 correctly leaves the historical scan: its latest publication belongs to normal delivery. Compacted rows are expected, not a defect.

Historical pagination has a fixed upper bound S and monotonically advances B. It terminates even under sustained publication because new positions are strictly above S. Do not chase a moving upper bound or rewind L. Each page uses the existing repeatable-read transaction and Loader/stamp coherence.

On the final historical page, capture its transaction's current server head H. Persist this as a fixed completion barrier. Complete only once B=S and L>=H. Any initial identity skipped because its publication moved above S before the final scan belongs to the subscription's delivery coverage through H. Republishing after H is ordinary later synchronization; completion does not await an unbounded stream of future changes.

This argument assumes retained invalidation identities, monotonic publication cursors, eventual delivery, and successful authoritative reads. Future retention #61 or current-membership removal #140 must preserve or replace that contract. No server schema changes, stable identity ordering, new host operation, or long-running snapshot transaction are needed.

## 4. Request/response and backend contract

Reuse `/sync/pull`, existing host `head` and cursor-ordered `scan`, and ordinary authority resolution. Dispatch an explicit bootstrap request before normal decoding; reject unknown modes.

```ts
type BootstrapRequest = {
  mode: "bootstrap";
  channel: string;
  models: Record<string, number>;
  after: number; // committed B
  until: number; // subscription origin S
};
type BootstrapPage = {
  mode: "bootstrap";
  channel: string;
  from: number;
  to: number;
  until: number;
  head: number; // current server head in this page transaction
  records: AuthorityRecord[];
};
```

Require safe counters and `0 <= after <= until <= head`. Echo the requested channel/from/until. Client rejects mismatches, backwards progress, a nonterminal page without progress, or more than 50 records. Completion is `to == until`; no redundant done flag is necessary on the wire.

Call the existing scan with `after=B, limit=50`. Validate all returned rows against normal increasing-cursor/head/identity rules. Only rows with cursor <= S enter Loader resolution. If the scan returns fewer than 50 rows, reaches S, or encounters a row above S, set `to=S`; otherwise set `to` to the last returned cursor. With B=S, return an empty terminal page with current head H. Historical records can disappear from this interval only by moving forward into subscription coverage.

Extract the existing grouped Loader/read-version/normalization helper for both pull modes. Preserve owner checks, undeclared Model failures, batch-read fallback, tombstones and per-record errors. PostgreSQL's existing scan and repeatable-read request wrapper suffice; test their use without changing the invalidation schema or introducing `first_cursor`.

## 5. Client ledger and page application

Extend the same #150 subscription row:

| Field | Meaning |
| --- | --- |
| `bootstrap_state` | `not_requested`, `requested`, `loading`, `catching_up`, `complete`, `failed` |
| `bootstrap_run` | monotonically incremented retry/run fence for SDK waiters and in-flight responses |
| `bootstrap_barrier` | final historical page head H, initially NULL |
| `bootstrap_cursor` | numeric historical progress B, initially zero |
| `bootstrap_error` | bounded JSON with code/message and the failed page's record reports, or NULL |

Do not duplicate `starting_cursor` or live `cursor`. Do not create a generic job table. The subscription identity plus bootstrap run and Engine request ID fence stale responses. Calls share an active run; explicit retry after failure increments the run but retains S and its last successfully committed historical cursor. Pending waiters remain attached to their original run, so a rapid retry cannot turn an earlier failed call into success.

Registration is a local transaction. A requested task with `starting_cursor=NULL` waits for #150 initialization. Otherwise the scheduler can issue its first/next page. In one local transaction, validate the expected epoch/run/progress, apply authority using `Engine::apply_records`, and update only bootstrap fields. Success commits data and progress together. Duplicate old pages cannot move progress backward.

If any delivered record fails to load, validate or apply, retain successful authority writes from the page, mark the run failed, and leave its continuation marker unchanged. Record the bounded failure report. No unrelated live work or Actions stop. A later explicit bootstrap call retries that same page; stamps make successfully applied records idempotent. Transport failures roll back the uncommitted page and use the Engine's bounded backoff, preserving pending work. They do not convert all rows into business failures.

After historical progress reaches S, store the final page head H and require ongoing `cursor >= H` before marking complete. H is fixed after the terminal page commits, not refreshed while waiting. If necessary wake the existing catch-up path; bootstrap does not synthesize cursor advancement. The transaction that observes both conditions marks complete and emits a committed status event, then SDK waiters resolve. A zero-head empty Scope can complete normally.

## 6. Coverage argument and limitations

For every identity published before S, either its retained position remains in the historical interval and Bootstrap reads it, or it moves above S and normal subscription delivery covers it. The final-page barrier joins the two paths. Record stamps prevent late historical data from replacing newer authority or deletion; pending optimistic edits replay under the existing rules.

Completion means the historical interval and the fixed live delivery barrier have been processed. It is not a global snapshot or a new guarantee overriding D7/D8: a live Loader/apply failure is still reported, does not count as synchronized authority, and does not silently become absence. Such failures may have occurred before bootstrap was requested; this milestone adds no durable per-record failure queue. A failure on a Bootstrap page rejects that run as specified above. Public documentation must distinguish completed loading/delivery from successful resolution of records for which the application received errors. Convergence retains the existing assumption of valid records and successful eventual reads.

The supported retained client store remains scoped to the same authenticated principal. Principal switching and arbitrary out-of-band SQL changes are outside this feature.

Unsubscribe removes initialization and bootstrap progress but retains Models/stamps/pending Actions (existing D6). Resubscribe creates a new identity and must establish coverage anew. Replica rebuild or incompatible read-contract changes reset coverage through the existing schema reconciliation path. Future eviction must invalidate affected completion evidence before dropping data (#139); arbitrary out-of-band SQL mutation is outside the public guarantee.

## 7. Scheduling and recovery

Use one Rust Bootstrap controller per client, with at most one bootstrap HTTP request in flight across Scopes in the first version. Rotate requested Scopes after each page, and release local transactions between pages. It is an Engine task lane, not another OS process or independent synchronization engine. The existing push and live lanes continue independently. Hosts execute requested I/O and report events; they do not own pagination, retries or completion decisions.

Persisted requested/loading/catching-up tasks resume after client reopen and connection setup. Every historical request retains the same S from the subscription. A terminal response lost before local commit may observe a newer H on retry. Once its barrier is committed, reconnect and repeated calls retain it. A stale response after unsubscribe, retry, close/reopen or schema rebuild cannot recreate or complete a replaced task.

## 8. Verification

Test bounded cursor scans with sparse/compacted rows, republishing across S, repeated updates during paging, empty intervals, and exact 50-record boundaries. Against real PostgreSQL, prove normal scan ordering and per-page repeatable-read content/stamp coherence. Test SQLite crash/reopen points and atomic page+progress commit. Exercise final-page H ahead of live progress, fixed H under ongoing writes, stale responses, late bootstrap versus live update/delete, overlapping Scopes, pending Actions, failed Loader/application records and explicit retry, callbacks throwing after commit, concurrent callers, offline initialization and foreground operations during large loads.

Include a test where a live Loader fails and L advances: its error remains visible and documentation must not claim that record successfully loaded when Bootstrap completes. Include a Bootstrap-page failure separately: successful writes remain, progress does not advance, the call rejects, and explicit retry revisits the page.

Provide one assembled client/server scenario exercising #150 and #151 together. Reuse #12 for separately reported 10k/100k/1m diagnostic scales; no capacity or latency claim follows from correctness tests. Runtime implementation is deferred until the written design/API is reviewed.
