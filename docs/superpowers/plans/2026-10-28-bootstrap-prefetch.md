# Bootstrap prefetch implementation plan

> Execute inline using the existing isolated worktree and test-first cycle.

**Goal:** Measure one-page Bootstrap lookahead independently of batch-size or
protocol-processing changes.

**Architecture:** Keep received request positions in controller staging only.
Derive a successor from one complete current Bootstrap page; preserve Store
authority, B/C, schema and settlement ownership.

## Steps

- [x] Add `protocol05_downlink/prefetch.rs` real SQLite controller scenarios.
  The initial assertion requires successor Pull before current Store apply;
  run it against the serial controller and observe its failure.
- [x] Add explicit opt-in controller construction and bounded successor
  scheduling in `crates/client/src/sync05/downlink.rs`. Retire future pages on
  failure/reconnect. Existing serial construction remains the default.
- [x] Add best-effort admission to `delivery_queue.rs`: validate normally,
  return a capacity decline before eviction/mutation, and preserve ordinary
  admission behavior. Cover capacity and malformed offers in unit tests.
- [x] Select the flag at open in `bindings/common/src/actor.rs`; record its
  selected value in performance implementation metadata. Do not alter the
  compatibility contract or public SDK/protocol fields.
- [x] Run client/sqlite tests and strict Clippy, then real host E2E with the
  experiment enabled. Update the owning downlink documentation.
- [x] Commit implementation, rebuild release native/Dart AOT, then run serial
  off/on comparisons at identical sizes/repetitions/instrumentation. Audit all
  samples and clocks, archive results and show a shared-axis timing figure.

Verification before measurement: client/sqlite 229 passed (7 ignored), strict
Clippy for client/sqlite/binding all targets passed, host E2E 21 passed with
prefetch enabled, and report/timeline Node tests 12 passed. Independent review
found an auth-refresh waiter retirement/duplicate-request path; three regressions
first reproduced it, then passed with the fix. Follow-up review found no remaining
issues. Implementation remains opt-in, with no merge or release in this experiment.

Measurement complete at clean `ab1c3bfb`, using identical binaries for off/on.
All 30 samples passed, and all clocks aligned. Initial 10k all-visible median
1971.997 → 1324.754 ms (-32.8%). Live single B-visible 27.552 → 35.683 ms
(+29.5%); 100-write all-visible 1673.770 → 1758.070 ms (+5.0%). These live
differences remain unattributed in five serial repetitions. The experiment stays
disabled by default. Full raw results, audit, selected shared-axis traces and
figure live in `integration/performance/baselines/2026-10-28-bootstrap-prefetch/`.

A follow-up interleaved live single-write check reused the exact binaries and
source fingerprint, five samples per condition, all ten passing. B-visible
medians 29.250 → 30.910 ms (+1.660 ms); four adjacent pairs favored on, one off,
with paired median -0.544 ms. The original +8.131 ms difference did not reproduce
stably; zero regression is not established, and the 100-write follow-up remains
unmeasured. No runtime fix was made, and no default enablement/merge/release is
authorized by this experiment. Its raw data, frozen runner, audit and selected
traces are archived beside the original campaign.
