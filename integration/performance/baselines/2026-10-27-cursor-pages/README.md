# Current-state cursor delivery — 2026-10-27

The same 45-case release matrix ran at `00a3f83aeecc7f469bf34ffd1236646de8419c8f` (dirty: `false`).
**40/45 samples passed; completed: `false`.**
[Workflow](https://github.com/zanminwang/axton/actions/runs/40603040161), [lossless raw samples](results.json.gz),
[evidence index](evidence.json), and [previous Store-only experiment](../2026-10-27-store-reads/README.md).

## Change

There is no server response-payload cache or expiry sweep. `saveDelivery`,
`readDelivery` and old-plan continuation fields are removed. The DDL retains five
canonical tables and drops the two retired payload tables while retaining
business/Store/Stream/outcome facts. Coordinated client/backend rollout is required.

Each request reauthorizes and reads current state above committed B/C. Simple
Models seek a bounded SQL page with whole-cursor ties/lookahead. Unique/cascade
Models retain complete metadata discovery for atomic dependency components.
A page targets 500 changes; oversized required components remain whole. Only
selected content is loaded. Its response carries every selected fragment and the
prefix actually covered, at or below the requested end. Independent versions
moved beyond that bound arrive in subsequent Sync; required ahead companions can
travel with the page without fabricating coverage. Clients atomically commit data
and B/C, then request a fresh page. Socket loss immediately retires incomplete
ordinary responses. Owned materialization returns a complete current owner proof.
Local incomplete-unit staging/digests remain; they are not server payload storage.

## Case outcomes

All attempted samples are included. Correct final data does not excuse an error.

| Scenario | Existing N | Writes M | Before passed | After passed | Backend errors | Connection errors |
| --- | --- | --- | --- | --- | --- | --- |
| initial-sync | 100 | 0 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| live-sync | 100 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| live-sync | 100 | 100 | 0/5 | 5/5 | 91 → 0 | 62 → 0 |
| initial-sync | 1000 | 0 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| live-sync | 1000 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| live-sync | 1000 | 100 | 0/5 | 5/5 | 66 → 0 | 61 → 0 |
| initial-sync | 10000 | 0 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| live-sync | 10000 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| live-sync | 10000 | 100 | 0/5 | 0/5 | 113 → 0 | 100 → 42 |

## Initial sync

Store-only → cursor pages, milliseconds, median [minimum, maximum]. Store open is
its own duration; visibility and Bootstrap start before open. Empty watch callbacks
are excluded from first visibility.

| Existing N | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- |
| 100 | 5.21 [4.99, 7.31] → 5.11 [4.97, 5.34] | 117.00 [114.10, 130.18] → 76.47 [73.61, 87.11] | 117.00 [114.10, 130.18] → 76.47 [73.61, 87.11] | 116.90 [113.93, 130.07] → 76.37 [73.51, 86.99] |
| 1000 | 5.03 [4.96, 5.32] → 5.19 [5.02, 5.32] | 278.08 [269.19, 298.12] → 163.03 [157.44, 172.90] | 360.69 [350.79, 379.44] → 280.74 [275.07, 297.61] | 360.54 [350.61, 379.27] → 280.55 [274.92, 297.40] |
| 10000 | 5.14 [4.92, 5.27] → 5.15 [4.83, 5.55] | 1453.33 [1409.19, 1646.72] → 162.89 [158.31, 164.00] | 3573.14 [3544.91, 3783.50] → 2991.33 [2972.92, 2997.33] | 3571.58 [3543.28, 3782.03] → 2990.19 [2971.52, 2996.01] |

## Continuous writes

Store-only → cursor pages, seconds, median [minimum, maximum] across all
client-completed runs, including failed ones. The previous burst samples failed
error criteria. They are diagnostic observations, not passing-baseline samples.
There are five independent runs per cell; writes within each run are correlated.

| Existing N | All local writes | All B visible | All A settled |
| --- | --- | --- | --- |
| 100 | 0.52 [0.45, 0.53] → 0.47 [0.46, 0.49] | 4.78 [4.49, 5.31] → 4.18 [4.12, 4.22] | 5.07 [4.45, 6.23] → 4.18 [4.12, 4.21] |
| 1000 | 0.50 [0.45, 0.56] → 0.49 [0.46, 0.51] | 5.60 [5.12, 6.42] → 4.85 [4.53, 4.90] | 5.97 [5.26, 6.20] → 4.84 [4.53, 4.89] |
| 10000 | 0.74 [0.63, 1.11] → 0.94 [0.76, 1.21] | 7.58 [5.94, 8.40] → 8.40 [8.23, 8.64] | 7.43 [6.54, 8.39] → 6.78 [6.72, 6.97] |

## Findings and verification

All 15 initial samples and all 15 single-write samples passed. Continuous
writes passed all ten runs at N=100/1,000. All five N=10,000 bursts failed solely
on B's `delivery queue overflow`: 12/8/7/6/9 events (42 total). The client's bounded
[queue](../../../../crates/client/src/sync05/delivery_queue.rs) is configured by
[downlink control](../../../../crates/client/src/sync05/downlink.rs) with 32 response
slots and one reserved for repair; this is a response-count
limit, not a PostgreSQL snapshot or row-lock error. No timeout, backend error or
failed Node host span was observed, and no server payload callbacks were invoked.

The previous experiment's 270 backend errors (including 165 delivery-payload SQL
failures) are absent. At N=10,000, first visibility improved 88.8% and full visibility
16.3% under the strict initial-only comparison. This is the combined effect of
removing persisted payloads, bounded candidate discovery and current-state paging;
the experiment does not isolate those contributions. Store open remains about 5 ms.

N=10,000 continuous-write B completion regressed from 7.58 to 8.40 seconds even
though A settlement improved from 7.43 to 6.78 seconds. Both sets contain failed
samples, so these are diagnostic timings. B's summed full-Model `watch.query` time
is 7,641 ms (median across five runs), versus 108.56 ms for B's `delivery.apply`.
`watch.publish` includes query work, and Dart bridge/drain overlaps notification
work: they cannot be added. The current [observer publication](../../../../crates/client/src/runtime/observers.rs)
reruns registered full queries when its stale mark is set and compares the
complete result before publishing. The [runtime commit mark](../../../../crates/client/src/runtime/mod.rs)
sets that stale flag on generation changes; it does not track which watched Model
changed. The 100-write runs at N=1,000 had a median 597 B watch queries; at
N=10,000, B had a median 260 queries amid overflow/recovery. Query count is not
equivalent to the number of newly visible Entries. Query work and consumer-side response backlog warrant a separate
backpressure/notification investigation; these traces do not establish their exact
causal contribution to overflow. Raising the queue limit was not used to make the
experiment pass.

At N=100/1,000 the 100 independent Creates still take 4.18/4.85 seconds to reach B,
while A submits local commits in 0.47/0.49 seconds. Across their 500 correlated
writes each, server transaction medians are 39.20/45.75 ms. Submission outpaces
ordered durable settlement. This is a separate throughput boundary, not evidence
that unrelated records contend on the removed global fence. Per-write server
commit confirmation is correlated for all 1,515 accepted writes; it uses the
server's own clock and is not subtracted from the Dart clock.

Representative completed stage sums, milliseconds, median per run. Initial spans
cover initialization; live spans begin at the measurement barrier. Stages nest or
run concurrently, and neither column is a wall-clock decomposition.

| Measured stage | Initial N=10,000 | Live N=10,000, M=100 |
| --- | --- | --- |
| Server transaction, all requests | 976.05 | 14777.30 |
| Server delivery planning | 923.09 | 5827.10 |
| Server canonical load | 93.26 | See raw stage index |
| Client B delivery apply | — | 108.56 |
| Client delivery apply, initial | 693.48 | — |
| Client B watch query | — | 7641.29 |
| Client watch query, initial | 620.81 | — |
| Client B bridge drain | — | 5579.60 |
| Client bridge drain, initial | 524.69 | — |

The evidence index preserves the full per-source/client summed stage medians.
Network one-way latency and transport versus notification time are not inferred
from unmatched clocks. The remaining failed case prevents calling this a clean
full-matrix baseline.

The new run has 45 client-completed samples,
45 exact server identity/count/content checks and
1515 accepted writes. Backend errors: 0; connection
errors: 42; nonterminal record reports: 1454.
Trace loss: 0; cleanup errors: 0; admin errors:
0. Backend/connection errors remain failures; only exact
verified accepted own-create divergence is admissible under the unchanged v2 rule.

Local verification passed: 517 workspace Rust tests (7 environment-dependent
ignored), strict Clippy, Rust formatting, TypeScript, Prettier and 22 Node tests.
The ten performance infrastructure tests passed again after archiving the result;
all 1,515 server commit correlations and all 45 report-admission checks were
revalidated from the compressed archive.

The [Linux full Verify job](https://github.com/zanminwang/axton/actions/runs/40603355098/job/122537199310)
passed at `9c3daa3a`. It covers initialized handshake/pull/live/materialization/
cursor-resume reads while a same-Store Mutation is suspended, duplicate upload
ordering/replay, cursor movement/removal authorization, generated Dart/JS native
E2E and actual PostgreSQL-to-SQLite 10k/100k capacity (debug correctness gate,
not these release latency samples). This validation commit differs from the
measured `00a3f83a` only by Prettier formatting in `persistence.mts`; the evidence
index records a matching normalized-source hash. The final five test-file
cleanups are also formatting only, with normalized equality checked.

The [macOS full Verify job](https://github.com/zanminwang/axton/actions/runs/40603348720/job/122536926548)
also passed at the same `9c3daa3a` commit, including the optimized-artifact gate.
The Linux and macOS evidence comes from separate push/PR jobs at identical source;
duplicate jobs were still running when the report was prepared. The final report
and formatting-only commit triggers its own workflow; those new job results are
not substituted for the completed code-validation evidence above.

Local PostgreSQL execution was blocked by sandbox shared-memory restrictions;
the real PostgreSQL evidence comes from GitHub Actions. The measurement workflow
exited nonzero as required for the five failed samples, and its artifact contains
the full attempted matrix.

The [superseded partial trial](superseded-4d670499.json.gz) retains three completed
N=100 initial samples at `4d670499`. It was deliberately cancelled to include the
socket-loss recovery fix. It is not combined with this final matrix.

## Origin audit

Persisted delivery plans were introduced in `19881cf7`, then integrated through
[PR #247](https://github.com/zanminwang/axton/pull/247), merged by `zanminwang`.
Git records the implementation author as Steve Wang. GitHub has no review
submissions on that PR. The [historical specification](https://github.com/zanminwang/axton/blob/54439785eeb8013b219f8d7071a5670a8bbeb375/docs/superpowers/specs/2026-10-07-internal-refactor-spec.md)
explicitly lists transient DeliveryPlan storage among recommendations introduced
by that specification and says they must not be described as earlier user
choices. The historical design review R1 likewise calls it an internal refinement,
not a previously approved detail. Authorship and merge-account metadata do not
prove personal design approval; these records do not identify a named approver or
agent. No claim is made about approvals outside the inspected records.

## Conditions and limits

Five serial repetitions per case; N=100/1,000/10,000, M=1/100, fixed 512-byte text,
full Entry watches, publication groups of 100, generated Dart/AOT and release native
artifacts. Processes, SQLite files and PostgreSQL databases are fresh. Compilation,
backend/database setup and fixtures are untimed. OS/database caches are not flushed.
Loopback shared GitHub VMs do not establish Most Days, phone, WAN, media or UI
latency. Raw metadata records commit/dirty, source/artifact hashes, tools and
PostgreSQL settings. Nested stages overlap and are not latency shares; clocks are
never subtracted across processes. No latency CI threshold is introduced.

| Condition | Store-only experiment | Cursor pages |
| --- | --- | --- |
| cpu | AMD EPYC 7763 64-Core Processor | AMD EPYC 7763 64-Core Processor |
| cpus | 4 | 4 |
| memoryBytes | 16766414848 | 16766414848 |
| release | 6.17.0-1022-azure | 6.17.0-1022-azure |
| node | v26.4.0 | v26.4.0 |
| dart | Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" | Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" |
| rustc | rustc 1.98.1 (48a229cea 2026-09-01) | rustc 1.98.1 (48a229cea 2026-09-01) |
| postgres | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit |

The report command accepted `compare before after --scenario initial-sync` with
matching machine/tool/database/cache/fixture/timing metadata. Its full-matrix
comparison correctly rejected failed samples. Burst before/after values above are
explicit diagnostic observations and do not bypass that comparison contract.
