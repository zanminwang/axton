# Bounded Bootstrap prefetch experiment

Try one-page lookahead during initial Bootstrap while retaining the current
500-row pages, protocol 5 wire format and complete-unit commit rules. This is
an opt-in native experiment, not a new public SDK option.

The controller may request the successor of one admitted complete Bootstrap
page while that page waits for or performs Store apply. Received-through is
only an HTTP request position: S/B/C and authority still advance exclusively
through successful Store commits. At most two Bootstrap pages (the current
page and one successor) may be staged/in flight. Ordinary live Sync and owned
materialization retain their existing policies.

Successor admission uses the existing queue budgets without evicting retained
work. If it cannot fit, discard that response and disable lookahead for this
client; serial repair resumes from committed B. Malformed responses still fail
normal validation. Apply failure and reconnect retire future Bootstrap pages;
close/context changes retain existing cancellation and epoch fences.

Select the experiment once at native client open with
`AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=1`. Default is off. Tests configure the
Rust controller explicitly, without mutating process environment.

Verify real SQLite progress, out-of-order arrival, bounds, multi-unit pages,
failure, reconnect and close. Compare off/on release native and Dart AOT runs
under identical instrumentation-3 conditions. Preserve every sample, verify
the same final rows, and generate actual shared-axis traces. No speedup is
assumed, and the experiment is not automatically merged or enabled by default.
