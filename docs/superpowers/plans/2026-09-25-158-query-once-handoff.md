# Implementation handoff: Query once snapshots (#158)

Implement [#158](https://github.com/zanminwang/axton/issues/158) from `codex/158-query-once`, based on `ceced50` (merged #157). The branch contains reviewed preparation documents only. On the original host, reuse `/Users/stevewang/.codex/worktrees/158-query-once/axton`; on another host, fetch it into an isolated checkout.

Read [spec](../specs/2026-09-25-158-query-once-design.md), [plan](2026-09-25-158-query-once.md), repository AGENTS and the issue workflow. The user selected complete result caching, replacing an earlier successful-call-marker/ensure proposal. Do not implement that superseded design. If delegating implementation, use Sol as requested.

## Non-negotiable decisions

- Invocation-time once on direct Queries only; ordinary calls remain fresh and independent. No schema annotation.
- Return the same complete typed snapshot content on reuse: scalars, Model snapshots, ordering, membership and pagination metadata. Decode independent objects per caller.
- Cache hits never reapply old Model authority. Scope updates maintain Models without mutating cached snapshots.
- Persist cache and successful response authority in one local transaction. Failures do not establish cache; successful empty results do. Refresh replaces only on success.
- Explicit refresh and per-Query/args invalidation; canonical store variants prevent store:false hits from pretending Models were stored. Explicit once still persists result when store:false.
- Rust owns canonical keys, generation fencing and flight decisions; hosts execute existing direct I/O and fan out results. Handle concurrency, close, offline reopen and invalidation races.
- Database is the cache/account isolation boundary; no inference of identity from access tokens. Schema changes invalidate the prior contract partition.
- No cache mode for Mutations/enqueue, no new transport endpoint, TTL, automatic freshness, query membership or Bootstrap changes.

## Execution and completion

Claim the issue, implement plan tasks with meaningful failing tests, update docs/examples, and record material changes in the issue. Resolve routine engineering choices yourself. Review the complete implementation, fix findings, run required checks, open a PR with Closes #158 and attach it to your task. Satisfy required CI and merge under the user's standing instruction for assigned implementation to complete review and merge. Do not stop at PR creation, bypass checks or claim unavailable tests passed. Report an actual blocker if one prevents completion.

This preparation request created documents and review only; this handoff is for the user's separately assigned agent and is not an execution dispatch.

## Parallel integration

#27 changes creation defaults and generated create inputs; preserve its complete Query result types. #151 owns Bootstrap/Downlink. Shared compiler, DDL, native bindings and SDK files require integrating the first merge before final verification. Neither feature semantically blocks this one. If another branch is still absent, say combined tests were not possible and the later merger must run them.

## Evidence to report

No Query-once implementation tests have been run during preparation. Run compiler/core/client/SQLite, native and TS/RN/Dart coverage, action end-to-end and full host gate per the plan. Include real network counts, persistent reopen, cache/authority atomic rollback and invalidation race evidence. Final report: PR, merge commit, actual tests and remaining limitations. Preserve unrelated workspace changes.
