# Existing Bootstrap prefetch enabled on the current build

The existing native switch `AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=1` enables
one-page lookahead for each client opened in that process. No protocol or
prefetch implementation changes were made. The reusable local performance
entry point is [prefetch-on.sh](../../prefetch-on.sh); it sets the environment
variable and invokes the existing performance runner. Native defaults remain
unchanged; application processes must receive the variable before opening the
client. This run does not change a deployed application's launch environment.

## Short off/on comparison

All four samples passed. Fresh N = 10,000 initial syncs ran off/on and on/off
pairs with identical source, release native artifacts and Dart AOT clients.
Mutation commit-group size 8 was held constant on the server. There are only two
observations per condition; this is directional local evidence, not a general
performance guarantee or a live-sync regression assessment.

| Milliseconds, median [min, max] | Prefetch off | Prefetch on |
| --- | ---: | ---: |
| First nonempty watch result | 58.466 [55.949, 60.983] | 56.673 [56.131, 57.215] |
| All 10,000 visible | 1013.753 [1007.905, 1019.600] | 661.436 [659.317, 663.554] |
| Store open | 2.085 [2.016, 2.154] | 2.089 [2.059, 2.118] |

The all-visible median fell 34.8%. Both adjacent pairs favored prefetch by
360.283 and 344.351 ms. Every sample still requested 20 complete pages and
published row counts 0/500/1000/.../10000. The audit verifies that all 19 successor
HTTP requests began before the previous page's apply finished with prefetch,
versus zero without it. SQLite commit remains the progress authority.

All final client/server identities and content passed, with no backend/transport,
trace-loss or cleanup errors. Common-axis traces aligned within ±33.5 µs. Native
spans remain elapsed time rather than CPU profiles; nested spans are not an
additive breakdown. This run measured SDK watch visibility, not UI rendering.

Apple M5 Max, macOS arm64, Node 24.19.0, Dart 3.12.1, Rust 1.98.1 and PostgreSQL
16.14; fresh client process, PostgreSQL database and SQLite files per sample.
Fixture preparation, process startup and compilation are outside timing. Node
fixture state and OS caches are reused; there is no simulated WAN. Full machine,
database settings, source and artifact fingerprints are retained in the archive.
The working tree was dirty at parent `294f356`. Later launch-script and evidence
documentation additions do not change the measured runtime.

## Reproduction

After the normal release/AOT performance build, against a disposable localhost
`DATABASE_URL`, with Node/Rust/Dart available on PATH:

```sh
cp integration/performance/baselines/2026-10-28-prefetch-current/runner.mjs \
  artifacts/performance/prefetch-current.mjs
node integration/performance/run.mjs --record-build
node artifacts/performance/prefetch-current.mjs --samples 2 --sizes 10000 \
  --writes 1 --timeout-ms 60000 --fail-fast on \
  --output artifacts/performance/prefetch-current.json
node integration/performance/baselines/2026-10-28-prefetch-current/audit.mjs
```

The frozen runner uses initial sync only and chooses the flag per sample.
`results.json.gz` retains every raw sample and source span. `audit.mjs` verifies
its frozen runner hash, complete successful matrix, final state, page/snapshot
counts, timing alignment and actual HTTP/apply overlap, then derives
`evidence.json`. Auditing reads the retained archive rather than the most recent
artifact output. The existing real SQLite prefetch regressions are the targeted
correctness check for ordering, failure, reconnect, close and refreshed auth.
