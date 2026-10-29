# Default scheduling verification

Bootstrap one-page prefetch is now enabled by default in core Control and native
client construction. `AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=0` explicitly disables
it; `1` explicitly enables it. The environment is read before opening a native
client, and pure-core callers can override with `with_bootstrap_prefetch`.

`createBackend` resolves `protocol5.experimentalMutationGroupSize` once and shares
the resolved size with recursive host publication scopes. The default is 8 when
the native addon exports `processBatchGroup` and the adapter advertises
`mutationCommitGroups`; otherwise omitted configuration preserves size 1.
Explicit size 1 retains per-Mutation commits. An explicit unsupported size above
1 still fails startup. Current native/PostgreSQL configurations require no flag.
The old option name is retained for API compatibility.

This changes the default backend durability boundary for capable adapters:
members run sequentially, business refusals use individual savepoints, and an
infrastructure failure rolls back the current group while retaining earlier
committed groups. See the [server contract](../../../../docs/engineering/architecture/server/protocol5.md#sequential-commit-groups).

## Executed verification

- Real PostgreSQL Batch tests: all 28 passed, including a new no-option test
  whose tenth Mutation fails, leaving exactly the first eight durable; retry
  preserves their distinct cursors and completes the remaining two. Existing
  explicit size-1, refusal, duplicate replay, process-exit and grouped rollback
  coverage also passed. The size-1 process-exit fixture was made explicit after
  its initial run correctly exercised the new group default instead.
- Startup/host/database/default tests: all 17 passed. The new default tests cover
  size 8, explicit size 1, missing native/adapter capability fallback, and refusal
  of an explicitly requested unsupported group size.
- Full real SQLite and native binding suites passed, including default-enabled
  prefetch ordering/failure/reconnect/auth coverage and explicit serial mode.
  Optional ignored tests were not run.
- Workspace TypeScript checking, Rust linting and formatting passed.

The release Node addon and Dart native library were rebuilt. One fresh 10k
initial sync ran with both environment overrides explicitly removed and the
fixture deferring group size to the backend. It verified all data, snapshots
0/500/.../10000, 20 complete applies, and all 19 successor HTTP requests starting
before their predecessor apply completed. No errors, trace loss or cleanup
failure occurred. This smoke run verifies defaults; it is not a performance
comparison, and other regression checks ran concurrently.

```sh
node integration/performance/baselines/2026-10-28-defaults/audit.mjs
```

`runner.mjs` is the frozen local one-sample runner; it contains absolute workspace
imports that must be relocated elsewhere. `results.json.gz` retains raw spans,
identity/content checks, source/artifact fingerprints and settings;
`evidence.json` is derived by the audit. Documentation additions after collection
do not change the measured runtime. The disposable database was stopped after
verification. Changes remain local and have not been published or deployed.
