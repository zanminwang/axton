# Query once result snapshots

Issue: [#158](https://github.com/zanminwang/axton/issues/158). Based on main `ceced50`, after merged Query/Mutation #157. This assignment prepares reviewed documents only; the user assigns implementation separately.

## Approved behavior and public API

`once` is an explicit call-site policy, not schema. It persists the complete successful Query result and reuses its value on subsequent matching calls. It does not merely record that loading happened, infer business coverage, or maintain live query membership.

```ts
const result = await client.queries.getTodos({ projectId }, { once: true });
const refreshed = await client.queries.getTodos(
  { projectId }, { once: true, refresh: true },
);
await client.queries.invalidate.getTodos({ projectId });
// Default remains an independent network request, with no cache read/write.
const fresh = await client.queries.getTodos({ projectId });
```

For a parameterless TS Query, pass `{}` as the existing generated API requires. Dart uses generated named `once: true` and `refresh: true` beside the existing typed store selector, and `client.queries.invalidate.getTodos(projectId: projectId)`. Use collision-safe names (like the existing outputStore rule) if business inputs occupy option names. Reserve `invalidate` in the Query namespace; it is a generated member, analogous to enqueue. Invalidation returns void after its local transaction commits, needs no network, and clears all store variants of this Query/argument set in the current contract namespace.

This first milestone applies to direct Query methods only. Mutations and `queries.enqueue` keep existing behavior and do not accept once/refresh; enforce both generated types and runtime validation. Queued cache hits would require a different durable Call lifecycle and are outside the approved complete-result direct API. `refresh: true` requires `once: true`; otherwise reject before I/O. A refresh always observes a network outcome, retains the prior cache on failure and replaces it on success. No automatic stale fallback.

The return type remains the Query's generated Output on hit, miss and refresh. Persist canonical wire values, including scalars, Model snapshots, list order/membership and pagination metadata. Decode a fresh independent output object per caller/hit; callers mutating arrays, Dates or returned objects must not mutate the saved result or another caller's result. Equality is decoded value equality, not object identity.

On a miss, the existing direct call generates a fresh call ID, invokes the Handler/Loaders, validates the response and applies authority under the existing store policy. Successful cache persistence occurs in the same local transaction as required authority application, including a result with no records. Resolve only after commit. Failed server outcomes, invalid responses or failed local commits are not cached. Existing isolated authority reports retain their existing reporting semantics; this cache does not turn a report into successful authority or change direct-call completion rules.

On a hit, return only the saved output. Do not issue a request, replay a receipt, reapply old records, notify Model watchers or advance subscription cursors. Scope updates and local edits change Models, not cached results. Local Models are the current local view; Query result is the previous request snapshot. A successful empty result is cacheable. A successful paginated invocation caches only that page.

## Key and identity lifecycle

Key by cache format revision, compiled client contract fingerprint, Query name/version, canonical normalized arguments, and canonical store policy. Argument normalization must be the existing Rust Query validation, not raw JS object ordering. Equivalent object order and normalized UUID/date values share keys; array order and meaningful nulls remain significant. Omitted store/true/equivalent all-true maps share a key. Store false or a meaningfully different selective policy gets a distinct variant: a non-storing cache hit must not satisfy a later request to load Models, and old snapshots must never be replayed to implement that distinction.

Hash a canonical key with a collision-resistant digest or persist its canonical components with exact comparison. Keep a readable name/version and normalized args separately if needed for invalidation. The contract fingerprint is derived deterministically from the complete compiled client schema; any schema change invalidates old cache partitions. This conservative rule also covers output Model read contracts and decoder shape. Reopen with the same contract retains cache; rebuild/reset starts empty. Do not carry result-cache rows into a rebuilt replica. Prune obsolete contract partitions on open so repeated schema changes do not accumulate unusable snapshots.

Cache ownership follows the local client database. It is not an authentication cache and cannot infer the backend principal from an opaque access token. Applications must use a separate local database per backend/account/tenant identity or discard the old database when changing identity; changing credentials on a shared file does not isolate existing Models or these results. Token refresh for the same identity does not clear results. Document this prerequisite explicitly, test independent databases, and do not claim automatic account-switch detection.

No TTL or automatic Scope freshness policy. Entries remain until explicit invalidation, successful replacement, schema partition cleanup or database reset. `store:false` controls Model materialization only; explicit once still persists the complete result snapshot, so it is not an ephemeral/no-disk request. Explain this beside the options. First-version cache size is application-managed through invalidation; automatic eviction remains separate retention work.

## Storage and concurrency

Add `axton_query_cache` as additive framework storage with logical columns: `key` primary key, `contract`, `name`, `version`, canonical `args`, canonical `store`, `generation` token, and nullable `result` TEXT. SQL NULL means no successful result; a JSON result such as `{}` or `null` is distinct. Invalidated rows may remain as empty tombstones to fence old responses. A random generation UUID avoids counter overflow; change it on invalidation. Create-if-absent must not rebuild or discard pending work in an existing database. No server table, protocol payload, mutation queue column or new transport endpoint is required.

A focused Rust coordinator owns normalization, cache lookup, generations and active flight identity. Suggested boundary:

```text
begin_query_once(name, version, args, store, refresh)
  -> Cached { result }
   | Join { flight_id }
   | Fetch { flight_id, request }
finish_query_once(flight_id, response) -> existing ApplyReport/completion
fail_query_once(flight_id) -> release matching active flight
invalidate_query_once(name, version, args) -> local commit
```

The runtime coordinator is memory-only for active requests; committed cache rows survive restart. Keep active flights by flight ID, with a join index keyed by (cache key, generation), so old and new generations can coexist after invalidation. Remove only the matching index/flight on completion. Fetch owns the exact prepared direct request. Completion must match that request and runtime lifetime; do not accept an arbitrary request/key combination. Keep the existing transaction guard even for cache hits/invalidation: generated Queries are unavailable within app-owned local transactions and must not bypass it via a captured client.

Rules:

- A valid snapshot and non-refresh once returns immediately, even if a refresh is in flight.
- Otherwise join one active request for the same key and generation, or create one. Concurrent refreshes coalesce; a refresh can join an already active uncached request of the same generation. Independent default Queries never join or populate this cache.
- Hosts map flight IDs to shared raw-result promises/futures, then decode separately for each caller. TS/React Native and Dart execute I/O and observe decisions; Rust owns the state/generation rules. Install the host flight inside the exclusive callback that receives Fetch, before releasing that callback to another begin request (or prove equivalent ordering); test this race. Do not wait until an outer asynchronous continuation to register it.
- Invalidation deletes the saved result and changes its generation. A later call starts or joins work in the new generation. An older in-flight result can still resolve its original callers and apply normal stamped authority, but cannot repopulate the invalidated cache. It cannot clear a newer active flight either.
- Failed refresh preserves the prior result; failed miss leaves no successful result. Report the failure to the requesting callers and release active state on every terminal path. Retrying once starts a fresh request/call ID unless an existing flight can be joined.
- Closing cancels observers and discards active flights; delayed I/O is rejected under existing connection/runtime fencing. After crash/reopen, completed cache is available offline; an incomplete request is not durable work and a cache miss needs a new direct request.
- Cache hits work with no network carrier. Therefore perform local lookup before the existing direct-availability check. Miss/refresh offline reports existing direct-unavailable/transport behavior; never enqueue implicitly.

Persist validated results with authority application in one Rust write closure. Do not cache only in the host after applying a response, which leaves a crash gap. Validate cached data under the current output contract when reading; a malformed cache row is invalidated and treated as a miss. Database read failures remain errors, not silent misses. Decoder bugs remain visible; never return a partial result.

## Tests and integration boundaries

Cover hit/miss/refresh/default semantics, empty/plain/scalar/Model/list/date outputs, option and argument normalization, mutation-safe decoding, no-authority-reapply, offline/reopen/reset, concurrent observers and invalidation fencing. Real SQLite tests verify cache and authority atomicity; real SDK integrations verify type parity and network counts. Forged cache calls to Mutations or enqueue must fail before side effects. Keep existing Model write/report/transaction guarantees and direct call-ID dedup intact.

#27 modifies create input generation, not Query output caching. Both touch compiler emit/validation and generated fixtures; integrate the first merge before final verification, preserving both. #151 owns Bootstrap/Downlink; this feature does not touch its scheduling, subscription ledger or coverage contracts. If Bootstrap is present, include its smoke/regression coverage when running the full host gate.
