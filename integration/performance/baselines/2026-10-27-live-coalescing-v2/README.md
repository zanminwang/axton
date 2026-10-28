# Live coalescing with progress during HTTP repair

The final release/Dart AOT run passed **45/45** samples at
`7e495814de0cfce65da3ae04e387f40944608790`. Both full host Verify jobs passed at this same
runtime commit. This report is dated 2026-10-27 in the experiment's local date;
the final Actions run completed on 2026-10-28 UTC.

- [Performance run](https://github.com/zanminwang/axton/actions/runs/40611658011)
- [Full Verify](https://github.com/zanminwang/axton/actions/runs/40611634843)
- [Raw JSON artifact](https://github.com/zanminwang/axton/actions/runs/40611658011/artifacts/12783856093)
- [Evidence index](evidence.json) and [complete summary transcript](summary.log)

## Final measurements

Each cell has five serial repetitions. Times below are medians, with min–max in
brackets. Initial sync starts from an empty SQLite Store. Live sync starts after
two clients initialize; B visibility means B's watch contains the new identity.
Compilation, backend startup, database creation and fixtures are outside timing.

| Existing Entries | Initial first visible | Initial all visible | One create visible on B | 100 creates all visible on B |
| ---: | ---: | ---: | ---: | ---: |
| 100 | 57.5 ms [56.9–95.6] | 57.5 ms [56.9–95.6] | 55.3 ms [51.9–60.3] | 3.91 s [3.64–6.70] |
| 1,000 | 128.5 ms [124.7–153.5] | 250.9 ms [219.8–334.0] | 67.0 ms [54.9–69.4] | 4.31 s [4.22–7.16] |
| 10,000 | 124.7 ms [123.2–153.6] | 2.57 s [2.44–2.70] | 135.8 ms [131.4–141.5] | 5.61 s [5.13–6.38] |

Store open medians were 4.662 / 4.336 / 4.579 ms for N=100 / 1,000 / 10,000.
Bootstrap medians were 57.451 / 250.821 / 2,569.902 ms. First-visible excludes
empty watch notifications. All-visible and Bootstrap are independent milestones;
their difference is not a separately measured stage.

At N=10,000/M=100 the median local commit per write was 3.772 ms, server
transaction duration 47.425 ms, A's accepted local settlement 4,490.221 ms and
B visibility 2,851.002 ms. These per-write observations are correlated within
a run. They are different from the whole burst's 5,607.765 ms B completion;
A settlement is not the server commit timestamp. Dart clients share one
Stopwatch; server source times remain in their own clock domain.

The passing runner requires all 1,515 writes to have accepted settlements,
correlated successful server transaction evidence and exact final data. It also
checks both client contents, PostgreSQL contents, pending queue state, backend
and connection errors, trace loss and cleanup. The 1,454 reported nonterminal
owned-create observations passed the existing exact-identity/content/settlement
checks; there were zero failed reports. These are not discarded failures.
The final evidence here comes from the completed runner transcript; the raw
artifact has not had an additional independent audit after download.

## What changed

The 32-slot bounded queue previously admitted every distinct live response.
When a complete ordinary response was waiting for local application, new
responses could fill the queue. The previous cursor-only matrix failed all
five 10,000-entry burst repetitions with 42 queue errors.

The client now validates new live offers and retains the greatest observed head
while ordinary payload is staged. It finishes admitted fragments, commits the
covered prefix atomically, then repairs from the committed cursor. Owned/schema
delivery and the existing memory/disk/slot bounds are preserved. Malformed offers
cannot advance the observed head.

The first coalescing experiment also treated an HTTP request in flight as busy.
This final refinement permits an eligible live prefix to commit while HTTP waits.
Future offers that cannot fill the missing prefix remain hints; late stale HTTP
replies cannot roll back committed content. A deterministic SQLite regression
holds HTTP open and verifies live progress and the late-reply boundary.

| Experiment | Passing cases | 10k/100 B completion median | Range |
| --- | ---: | ---: | ---: |
| [Cursor pages](../2026-10-27-cursor-pages/README.md) | 40/45 | 8.40 s | 8.23–8.64 s |
| [First coalescing](../2026-10-27-live-coalescing/README.md) | 45/45 | 6.61 s | 6.45–6.64 s |
| Final live-prefix refinement | 45/45 | 5.61 s | 5.13–6.38 s |

These are observations from separate runs, **not a strict compatible-result
comparison or a causal percentage improvement**. The cursor-only run failed and
cannot be admitted by full-matrix comparison. The final artifact's machine/tool
metadata has not yet been unpacked for compatibility checking. Unaffected
single-write and initial metrics also changed between runs, and shared-runner
tails are wide. No samples were filtered.

This is receiver-side coalescing, not full transport flow control: servers may
still transmit live payloads that are reduced to head hints at the client.

## Remaining bottlenecks

Measured stage totals at N=10,000/M=100:

| Stage | Median summed duration per run |
| --- | ---: |
| Server delivery planning | 5.187 s |
| Server Mutation execution | 4.253 s |
| Local complete-unit apply | 0.532 s |
| Watch query, conversion and comparison | 3.967 s |
| Dart bridge drain/decode | 2.504 s |

These spans are nested and overlap. Live totals include both clients. They
cannot be added to explain the 5.608 s wall time or divided into exclusive
percentages. Mutation execution is measured inside the server operation;
the adapter's transaction duration separately includes external BEGIN/COMMIT.

The queue was the direct overflow cause and contributed to redundant application
and watch work. It was not the whole latency problem. One hundred independent
writes still incur per-Mutation server execution and transaction work. The
full-Entry watch re-reads and converts retained data after invalidation, so its
cost grows with N. Next investigations should separate host/database round-trip
cost from business-handler cost and inspect avoidable watch invalidations.
This change does not alter Mutation batching or watch invalidation.

At N=10,000 initial sync, measured planning/apply/watch/bridge summed medians
were 731 / 658 / 545 / 431 ms respectively. Their overlap prevents treating the
unaccounted wall time as measured network cost.

## Validation and reproducibility

Final runtime: 520 local workspace Rust tests passed, seven environment-specific
tests ignored; strict workspace Clippy passed. The 38 SQLite downlink cases cover
100 overlapping offers, stalled application, committed-cursor recovery, malformed
hints, complete fragmented units and held-HTTP live progress. Full Linux and
macOS Verify passed remotely at the measured source commit, including SDK/native
and real PostgreSQL coverage.

Each experiment uses 512-byte text, fresh processes, databases and Stores.
Fixtures are freshly written; OS and PostgreSQL caches are not flushed. This is
loopback on a shared Actions runner, not a cold hardware cache or WAN/phone
measurement. No performance threshold is introduced. Five repetitions do not
support a run-level p95 claim.

The execution environment disconnected before the final artifact could be
downloaded and unpacked. Consequently this directory preserves the completed
summary and artifact/evidence index, **not a local copy of final raw samples**.
The uploaded JSON contains samples, traces, failures/timeouts, source/dirty state,
artifact fingerprints and environment versions. The GitHub-reported archive
digest is `sha256:5fd7c63232e37e1e695cc4fc99ddd5b2239cd3455f5d3a0640fea1f96bf2546c`; it has not been independently rehashed here.
The artifact expires on **2027-01-26**. The first coalescing run's compressed raw
JSON is already archived permanently in the adjacent directory.

When an execution environment is available, download and archive the final JSON
before expiry, then use the existing strict comparison without relaxing its
compatibility checks:

```sh
gh run download 40611658011 --repo zanminwang/axton \
  --name sync-performance --dir artifacts/performance/live-coalescing-v2
node integration/performance/report.mjs artifacts/performance/live-coalescing-v2/results.json
node integration/performance/report.mjs compare \
  integration/performance/baselines/2026-10-27-live-coalescing/results.json.gz \
  artifacts/performance/live-coalescing-v2/results.json
```

No second comparison is claimed until that command succeeds. A metadata mismatch
must remain a refusal, not be overridden.
