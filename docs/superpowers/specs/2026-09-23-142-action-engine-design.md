# Durable and request-response Action execution — design (#142)

Status: implementation design under review. Depends on the completed #145 and #141 branches. Issue: https://github.com/zanminwang/axton/issues/142.

## Goal and boundaries

Execute one schema-defined Action through durable client submission or direct request-response, with the same application-owned backend handler and typed output contract. Business database changes and their replayable completion result commit in one backend transaction. The Rust core owns validation, scheduling, completion and reconciliation; SDKs adapt transport, language values and in-memory observers.

The implementation preserves the existing optimistic write and receipt settlement guarantees. It does not introduce cross-Action atomic groups, invocation-dependency APIs, cancellation methods, background jobs, external-effect outboxes or tool exposure. Existing declared Model-input prerequisites and sequence rules remain. Application effects outside the shared database transaction are the application's responsibility.

#116 owns the broader independent Model-output/materialization feature. The #142 execution spine must carry separate per-call result and authority payloads, support implicit mutation results through the existing readback path, and provide the shared resolver boundary used by explicit Model results. Implement the basic identity-to-Loader integration needed by the approved Action examples; leave new ephemeral policy, projection/query features and their dedicated acceptance in #116. Record this shared-core boundary back to both issues rather than claiming #116 is complete.

## Public behavior

- The default generated client.actions.name(args) returns an ActionCall after canonical inputs, derived optimistic operations and the queue record have committed together. Ordinary-only Actions create a queue record without Model changes.
- client.actions.call.name(args) creates a fresh invocation identity and sends a direct request without local durable enqueue or optimism. It resolves after the server result has been validated and required local authority processing has committed.
- Both entry points are invalid from an application-owned local transaction callback. Preserve #145's language-specific context guard and local-only transaction surface.
- ActionCall exposes status (pending/succeeded/failed) and wait only. Wait returns the agreed success/error outcome; initial submission and direct-call failures reject/throw. Server-confirmed business failure is distinguished in error metadata from transport/result-observation failure with unknown execution outcome.
- call IDs are internal correlation identities; no new public call.id method is required. A new invocation generates a new UUID once, and every retry reuses it.
- Direct requests do not wait for the durable queue. The durable batch lane retains one in-flight batch and contiguous sequence numbers. Direct requests claim their own call records and do not take the per-client durable batch lock merely to execute.

## Client persistence

Keep the existing pending queue and Model operation child tables to avoid replacing proven replay logic. Add call_id TEXT and args TEXT to pending invocation rows. For new Action rows call_id is unique/non-null and args is canonical normalized JSON. Retain ordinal, name, version, push assignment and divergence metadata; ordinal remains local ordering, call_id identifies the invocation across transport retries. During internal transition, legacy fixtures may omit the new columns, but all new public Action submission must populate both. No legacy database upgrade compatibility is required.

The canonical args object is the source of request inputs. Derived create/update/delete rows remain the source of local optimistic replay. Generate those rows once from the same validated args within the same transaction. Do not permit an input/operation mismatch to enter the queue. Optional Model operands normalize to null; ordinary nullable parameters are required keys with nullable values; omitted update patch fields remain omitted. Input arguments and read-contract declarations become immutable once a batch freezes.

Preserve the existing per-record authority skip/report semantics during reconciliation. A locally skipped authority record is reported through the existing apply diagnostics, not silently turned into a business rejection or a second handler execution. Completion is delivered only after that processing transaction commits; the returned Loader snapshot remains independent of whether a newer local base made its authority stale.

No successful business result is stored in a local result-history table. Receipt settlement atomically removes completed queue rows and updates completion/deduplication metadata. Preserve the existing durable rejection inbox, including failures when no call handle survives. Existing last-completed batch tracking remains authoritative for durable settlement; direct responses do not advance it or Channel cursors.

## Protocol and outcomes

Represent invocation intent as {callId,name,version,args}, with the relevant declared Model read versions in the request envelope. Each result Model uses the read version retained in its Action-version output descriptor; authority reconciliation uses the caller's supported local Model read contract. Validate both independently and do not silently substitute the newest Loader shape for a retained Action result. Reuse a loaded snapshot only when identity and read version both match. Ordinary-only schemas may declare an empty models map. A durable envelope retains clientId, batchSequence and per-call ordinal; a direct envelope contains one call plus models and no durable sequence.

Each completion contains a callId and a tagged outcome: success with the final named result, or failure with a stable error code and execution classification. Encode a void result as null on the wire and map it to the language's void/unit value using the descriptor. A nullable named output remains a field in the result object, so it is not confused with void.

A batch receipt carries both outcomes and authority records. Outcomes are correlated one-to-one with the frozen calls, with no duplicate/missing/unexpected call IDs; rejected ordinals and failure outcomes must agree. Authority records may still collapse to the final stamp/content for each record. Per-call result snapshots must not collapse: if two calls load A and B for the same Todo, their results remain A and B while final settlement can carry only authority B.

Retain request byte limits and the current batch size limit. Validate envelopes, call IDs, descriptor versions, named result fields and Model snapshots before local settlement. If a response cannot be decoded or correlated, retain the frozen queue and report an infrastructure/protocol fault rather than confirming success or fabricating business rejection.

## Backend persistence and execution

Add an ahead_call table in the application's PostgreSQL database:

    owner_id TEXT NOT NULL
    call_id TEXT NOT NULL
    request TEXT NOT NULL
    response TEXT
    PRIMARY KEY (owner_id, call_id)

The stored request is canonical immutable intent, including name, version, args and Model read-contract versions, excluding transient batch position. It prevents accidental reuse of the same ID for different intent. A mismatched request produces call.identity_conflict and never executes the new body. Do not restore the removed batch request_hash; this check belongs to the new per-call contract.

claimCall inserts the row if absent, then SELECT FOR UPDATE locks it in the same transaction used by business writes. A concurrent duplicate waits and then observes the stored response. A null response is only an in-progress row inside that uncommitted transaction; normal execution must not commit a placeholder. saveCall writes the canonical complete response, checked against owner/call identity. A fault rolls back the placeholder, business writes, stamps and publications together.

For a durable batch, retain the outer client claim/sequence validation and per-call savepoints. Claim each call before its business savepoint. If a saved response exists, replay it without handler or Loader execution. Otherwise run the handler and result processing; a per-call rejection rolls back that call's savepoint, then stores its failure outcome outside that savepoint but inside the batch transaction. Infrastructure transaction failure rolls back the whole delivery. Save the batch receipt and per-call outcomes before the outer commit. Reply and wake Channel listeners only after commit.

A direct call runs the same execute-call function inside its own backend transaction, with no client batch claim. It still receives Model authority for successful mutation inputs and reported extra changes. Existing per-call snapshot read/stamp consistency must hold even when another direct/durable transaction touches the same record.

Version one performs no automatic pruning of backend call responses. This avoids making old IDs executable again while the retention protocol is unimplemented. Document storage growth and introduce a separate reviewed retention/tombstone policy before adding automatic deletion. Tests must not assume a TTL or permit an expired response to restart a completed effect.

## Handler and Loader pipeline

Generated handlers receive {ctx,args}. ctx contains the current application transaction, authenticated owner, stable call ID and the existing changes/publication collectors; the application supplies business logic and database access. Preserve one handler registration per Action version for both entry points. Generated args are the approved flattened Model inputs and ordinary values, not old internal data/patch wrappers.

Handler returns explicit ordinary values and Model identity objects. For relatedTodo Todo?, it returns {relatedTodo:{id:...}} or {relatedTodo:null}. The engine validates the object, extracts its identity and invokes the appropriate versioned Loader. Implicit create/update output identities come from their inputs; delete outputs are input identity confirmations. The handler cannot replace an implicit identity.

Required changed-record readback, stamps and publications retain the shared transaction path. Reuse a changed record's loaded snapshot when an output references that same identity/read version; additional read-only Model outputs use the Loader plus consistent existing/initialized stamp evidence without falsely advancing a business-change stamp. Preserve result ordering and multiplicity even if backend reads are deduplicated.

Validate the full named output contract. Missing required outputs, invalid scalars/enums, wrong identity objects and missing non-null Models fail that call's result processing and roll back its transactional business changes. Nullable missing Models may return null; authorization/read failures remain errors. A null field without a selected identity is not deletion evidence. Apply stamped authority through the shared Rust path, never via SDK-specific database mutation.

## SDK observation and lifecycle

The Rust settlement/direct-application result includes transient completion events after its local transaction commits. TS/RN and Dart registries map callId to weak observation states. They do not persist the business result or reinterpret Rust settlement outcomes.

Each live handle owns its state and settled result. Calling wait registers a strong active-waiter retention until completion, so awaiting the returned promise/future continues to work even if the wrapper variable is dropped. Multiple waits share one outcome and never repeat a network operation. Merely creating a call does not make the registry a permanent strong owner.

On registration and response processing, scan weak entries and delete dead entries. Completion explicitly removes routing and active-waiter retention after resolving observers. A live handle continues to own the result for repeated wait; unobserved results are discarded after required settlement. No timer or finalizer callback is required. Tests use injectable weak-reference access or explicit registry states rather than depending on garbage collector timing.

Closing the client stops transports, clears routing and resolves active waits with client.closed / execution unknown; queued work remains durable and resumes on reopen. Do not restore old handle objects or add public historical-result lookup. Waiting on a handle after close returns the same observation failure, not a new execution.

## Transport and failures

Use /sync/actions for direct request-response; retain the existing durable push transport while extending its envelope. Rust defines request construction, correlation and local application. SDK transport performs bytes-in/bytes-out with authentication and abort signals; direct HTTP awaiting must not hold the local database exclusive lock.

Durable transient transport/infrastructure faults reuse the existing retry/backoff and frozen request identity; they remain pending. Server-confirmed Action failures complete as failed outcomes and retain rejection visibility. Direct calls do one request attempt, with existing authentication refresh if configured using the same identity; they do not become a durable queue item on failure. Use existing request timeout configuration where present; otherwise impose a finite 30-second per-attempt direct timeout with a documented transport option. A timeout or lost response produces execution unknown, never a claim that the transaction rolled back. No public cancel method is added.

## Evidence and acceptance

Tests must demonstrate atomic queue/optimism submission, ordinary-only offline persistence, reopen recovery, concurrent server deduplication, rollback before commit, response loss after commit, identical replayed output, two per-call snapshots versus one final authority, direct calls independent of queued work, local materialization before success, rejection isolation, wrapper-independent pending waits and sweep cleanup. Use real SQLite for queue/reopen and PostgreSQL for transactional claims/receipts. Preserve existing simulation and receipt/page ordering tests. TypeScript, React Native and Dart must implement the same contract. Protocol/host fixture round trips and generated examples are required. Do not claim #116's deferred ephemeral acceptance or device smoke tests have passed without running them.
