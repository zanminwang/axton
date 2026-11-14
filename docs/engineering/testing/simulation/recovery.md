# Failure and recovery

[scenario.rs](../../../../crates/sim/src/scenario.rs) reopens the same SQLite file after Queued, Frozen, SavedAcceptance, Covered and Settled boundaries. [scenario.rs](../../../../crates/sim/tests/scenario.rs) permutes receipt/authority order and private versus Stream targets, then asserts exact retry bytes and durable completion. Close/reopen does not establish arbitrary process-kill or interrupted-commit durability.

The pure progress scenarios in [scenario_coverage.rs](../../../../crates/sim/tests/scenario_coverage.rs) serialize incomplete staging, test expiry without progress and verify replacement delivery. They do not inject partial network bytes or database failure.

Actual SQLite process-exit cases belong to [settlement.rs](../../../../crates/sqlite/tests/settlement.rs); joined HTTP/PostgreSQL behavior belongs to [adoption gates](../../protocol5-adoption.md). No current action-list shrinker exists. Preserve the tested commit, named scenario and fixture/order when reproducing failures.

Run `cargo test -p axton-sim --test scenario --locked`.

Evidence below was inspected on 2026-10-09; these suites were not executed for this documentation change. Prior execution records remain in [history](../../history/pre-protocol5/testing/simulation/recovery.md). Source inspection supplies neither a new passing result nor a complete-coverage claim.
