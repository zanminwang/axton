# Coalesced live offers, initial HTTP-wait policy — 2026-10-27

The same 45-case release matrix ran at `ecf5a21a2d661cf38bf9ab8bf61a60c61f9b8067` (dirty: `false`).
**45/45 samples passed; completed: `true`.**
[Workflow](https://github.com/zanminwang/axton/actions/runs/40609260912), [lossless raw samples](results.json.gz),
[evidence index](evidence.json), and [previous cursor-page experiment](../2026-10-27-cursor-pages/README.md).

## Change

The client admits an idle live offer directly. While ordinary delivery is staged
or an HTTP cursor request is outstanding, new socket responses pass the same
Store/context, expiry and fragment validation but retain only their highest head.
No payload is queued for those offers and no cursor advances. After current work
commits, one HTTP cursor pull requests current state from durable B/C. Fragments
of an admitted response still complete its atomic unit. Owned schema/settlement
obligations are not coalesced away. Queue slots, memory/disk limits, watch workload,
server execution and Mutation Batch policy are unchanged. `delivery.coalesce`
spans expose the normal hints. Invalid offers remain errors.

This is receiver-side demand scheduling and coalescing. The server still sends
live payloads, so it does not establish server/transport backpressure or eliminate
transmission cost. It adds no persisted snapshot or server cache.

## Continuous writes

Previous cursor pages → coalescing. Seconds, median [minimum, maximum] of five
independent client-completed runs per cell, including failed ones. Previous
N=10,000 runs failed error criteria; their timings are diagnostic observations.

| Existing N | Passed | Connection errors | All A local commits | All A settled | All B visible |
| --- | --- | --- | --- | --- | --- |
| 100 | 5/5 → 5/5 | 0 → 0 | 0.47 [0.46, 0.49] → 0.47 [0.47, 0.48] | 4.18 [4.12, 4.21] → 4.21 [4.19, 4.30] | 4.18 [4.12, 4.22] → 4.21 [4.17, 4.29] |
| 1000 | 5/5 → 5/5 | 0 → 0 | 0.49 [0.46, 0.51] → 0.51 [0.50, 0.52] | 4.84 [4.53, 4.89] → 4.92 [4.51, 5.03] | 4.85 [4.53, 4.90] → 4.91 [4.50, 5.02] |
| 10000 | 0/5 → 5/5 | 42 → 0 | 0.94 [0.76, 1.21] → 0.89 [0.74, 1.07] | 6.78 [6.72, 6.97] → 6.46 [6.31, 6.53] | 8.40 [8.23, 8.64] → 6.61 [6.45, 6.64] |

## Receiver work

Median completed span count or summed milliseconds per run, B only. Previous →
coalescing. These spans overlap/nest and do not decompose end-to-end latency.
`watch.query` includes query result conversion/comparison; it is not pure SQL time.

| Existing N | Watch query count | Watch query ms | Apply count | Apply ms | Coalesced offers |
| --- | --- | --- | --- | --- | --- |
| 100 | 597.00 → 600.00 | 212.02 → 222.14 | 100.00 → 100.00 | 111.06 → 110.30 | 0.00 → 0.00 |
| 1000 | 597.00 → 598.00 | 1320.46 → 1346.81 | 100.00 → 100.00 | 122.76 → 115.25 | 0.00 → 0.00 |
| 10000 | 260.00 → 154.00 | 7641.29 → 4820.19 | 83.00 → 49.00 | 108.56 → 83.97 | 0.00 → 97.00 |

## Single creates

Milliseconds, median [minimum, maximum], five runs per size. Local commit,
settlement and B visibility start at the same Dart write-start boundary. Server
transaction duration uses only the server clock and is not an A-to-server latency.

| Existing N | Passed | A local commit | Server transaction | A settlement | B visible |
| --- | --- | --- | --- | --- | --- |
| 100 | 5/5 | 1.57 [1.23, 1.66] → 1.31 [1.23, 2.37] | 31.42 [29.54, 31.62] → 29.89 [28.89, 31.16] | 72.01 [68.48, 78.84] → 78.66 [77.22, 83.81] | 72.07 [68.88, 72.48] → 71.08 [63.74, 81.38] |
| 1000 | 5/5 | 2.97 [1.37, 3.03] → 2.91 [1.42, 3.12] | 32.80 [31.87, 32.97] → 33.14 [32.50, 34.94] | 68.55 [67.57, 71.94] → 83.06 [74.68, 84.58] | 80.90 [78.64, 84.40] → 82.26 [78.88, 85.23] |
| 10000 | 5/5 | 1.56 [1.49, 1.60] → 1.55 [1.42, 1.60] | 34.82 [34.13, 35.12] → 34.86 [33.92, 35.32] | 72.92 [67.92, 73.56] → 78.62 [77.40, 83.31] | 163.40 [156.27, 166.56] → 155.44 [152.87, 165.73] |

## Initial sync

Milliseconds, median [minimum, maximum], five runs per size. First/all visibility
and Bootstrap begin before Store open; empty watch callbacks are excluded.

| Existing N | Passed | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- | --- |
| 100 | 5/5 | 5.11 [4.97, 5.34] → 5.22 [4.93, 9.44] | 76.47 [73.61, 87.11] → 76.99 [73.08, 94.17] | 76.47 [73.61, 87.11] → 76.99 [73.08, 94.17] | 76.37 [73.51, 86.99] → 76.89 [73.00, 94.02] |
| 1000 | 5/5 | 5.19 [5.02, 5.32] → 5.04 [5.01, 6.21] | 163.03 [157.44, 172.90] → 169.55 [160.21, 174.69] | 280.74 [275.07, 297.61] → 291.11 [279.78, 292.18] | 280.55 [274.92, 297.40] → 290.98 [279.67, 291.95] |
| 10000 | 5/5 | 5.15 [4.83, 5.55] → 5.27 [5.04, 5.40] | 162.89 [158.31, 164.00] → 157.96 [155.33, 159.44] | 2991.33 [2972.92, 2997.33] → 2943.86 [2871.89, 2953.97] | 2990.19 [2971.52, 2996.01] → 2942.18 [2870.15, 2952.57] |

## Findings

All 45 samples passed; the previous five N=10,000 burst failures and their 42
queue-overflow errors are absent. B completion fell from 8.40 to 6.61 seconds
(21.4% lower observed median). B watch-query count fell from 260 to 154 and
summed query/conversion/comparison time from 7.64 to 4.82 seconds. B still used
49 apply spans; coalescing does not mean a single 100-record apply. The median
97 hint-only offers includes repeated/current observations and cannot be added
to apply counts to recover exactly 100 writes.

At N=100/1,000 the receiver admitted every write directly and recorded no B
coalescing; their burst durations stayed near 4.2/4.9 seconds. At N=10,000 A's
all-settled time is still 6.46 seconds, near B's 6.61 seconds. This intervention
fixes normal receiver overflow and removes some delay; it does not remove the
ordered per-Mutation transaction/settlement path. Nested watch and server spans
cannot be added into percentages of end-to-end time.

Single-create A settlements shifted from 72/69/73 ms to 79/83/79 ms, while B
visibility stayed similar. The initial policy coalesced one A live offer per
single-create sample while HTTP was in flight. Code inspection identified a
liveness boundary: an eligible live prefix could wait behind a held HTTP request
even with no ordinary payload staged. The subsequent policy refinement admits
that live response. This archive is retained unchanged and does not measure the
refinement; a separate run must establish its timings.

Final exact server identity/count/content checks: 45/45;
client-completed runs: 45; accepted writes: 1515;
correlated server commit confirmations: 1515. Backend errors:
0; connection errors: 0; nonterminal
record reports: 1457; trace loss: 0; cleanup errors:
0; admin errors: 0. The unchanged v2 report
rule admits only an exact verified accepted own-create transient divergence.
Errors, failed samples and timeouts are never discarded to improve statistics.

## Validation

519 local workspace Rust tests passed (7 environment-dependent ignored). The 37
affected downlink tests passed again after the final admission-order change.
Strict workspace Clippy and 42 Node contract/effects/driver/startup/diagnostics
tests passed. Deterministic SQLite regressions pause the Store worker through
100 live offers, then verify committed-cursor repair, exact identities/latest
content, coalescing during HTTP repair, atomic completion of admitted fragments,
and rejection of malformed offers without poisoning the head.

Local JS package generation was blocked by sandbox process-identity access;
the Dart build hook was blocked by user telemetry-file permissions. Those setup
failures are not successful SDK validation. The direct Node addon rebuilt.
PostgreSQL cannot start locally under the sandbox shared-memory restrictions.

The [first full Verify](https://github.com/zanminwang/axton/actions/runs/40609252450)
failed one JS test whose HTTP fixture contradicted its socket state when the new
policy permitted cursor repair. The [fixture-only correction](https://github.com/zanminwang/axton/commit/e55dc65f31bccafd7e7a0c4842e15578e8b7c12c)
keeps exact data and atomic-watch assertions and checks committed-cursor repair.
The evidence index proves the validation revision changes only this test file;
measured runtime source is identical. Its full Verify was still running when
this initial-policy archive was prepared; no full SDK success is claimed here.
A local direct-addon SDK attempt also hit the protected application-data Store
lock directory, so it is not counted as a passing functional SDK run.

## Conditions and limits

Same generated Dart/AOT, release native artifacts, 512-byte text, full Entry
watches, N=100/1,000/10,000 and M=1/100, five serial repetitions. Build, setup and
fixtures are untimed. Each sample has fresh processes, SQLite files and a temporary
PostgreSQL database; OS/database caches are not flushed. There is no performance
CI threshold. Shared loopback VMs do not establish WAN, phone or Most Days UI time.
Per-write observations within a run are correlated; five runs do not support an
independent-run p95. Source stages are not summed into a wall-clock breakdown and
Dart/Node/native clocks are never directly subtracted.

Machine/tool/database/cache/fixture/timing metadata matched the previous run;
strict initial-only comparison was admitted. Whole live/full-matrix comparisons
correctly refused the previous failed samples. The burst before/after numbers
are descriptive diagnostic observations, not a bypass of that comparison rule.
No old failure or transient report was removed to produce the 45/45 result.
