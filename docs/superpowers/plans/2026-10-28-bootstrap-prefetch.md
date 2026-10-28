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
- [ ] Commit implementation, rebuild release native/Dart AOT, then run serial
  off/on comparisons at identical sizes/repetitions/instrumentation. Audit all
  samples and clocks, archive results and show a shared-axis timing figure.

Verification before measurement: client/sqlite 229 passed (7 ignored), strict
Clippy for client/sqlite/binding all targets passed, host E2E 21 passed with
prefetch enabled, and report/timeline Node tests 12 passed. Independent review
found an auth-refresh waiter retirement/duplicate-request path; three regressions
first reproduced it, then passed with the fix. Follow-up review found no remaining
issues. Implementation remains opt-in, with no merge or release in this experiment.
