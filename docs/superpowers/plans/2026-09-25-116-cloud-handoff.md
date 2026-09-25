# Issue #116 implementation handoff

Implement [#116](https://github.com/zanminwang/axton/issues/116) from branch `codex/116-ephemeral-outputs`, following the [approved spec](../specs/2026-09-25-116-ephemeral-outputs-design.md) and [implementation plan](2026-09-25-116-ephemeral-outputs.md). These documents are preparation only; implementation remains outstanding.

## Authoritative decision

The public option is **store**, selected when calling the Action:

```ts
await client.actions.call.searchTodos({ query }, { store: false });
const call = await client.actions.searchTodos({ query }, { store: false });
const outcome = await call.wait();
// Mixed outputs: { store: { suggestions: false } }
```

Default is true. Do not implement schema @ephemeral, a materialize option, or backend-deployed policy selection from older comments/plans. No policy-specific Action version bump is required. Handler/result contracts stay unchanged. Disabled output storage must never remove required mutation or handler-extra-change reconciliation. Policy is part of the durable invocation, frozen request and canonical call identity; saved results/authority replay unchanged.

## Assignment

1. Read repository AGENTS.md, issue history, architecture/guarantees, testing guide and the revised spec/plan. Where older issue comments conflict, this revision supersedes them.
2. Reuse or create an isolated worktree. Fetch current main and integrate concurrent work safely before implementation; preserve #150/#151 Downlink and Bootstrap ownership. The preparation branch contains only documentation.
3. Implement the plan continuously through Rust, durable storage/protocol, resolver, generated TS/Dart SDKs, bindings, integration tests and docs. Routine implementation decisions are delegated. Do not stop after another planning pass.
4. Run the relevant checks and full host gate with real database evidence. Review the final diff and PR yourself before merge, fix every actionable finding and rerun affected checks. Report unavailable checks honestly.
5. Create a PR closing #116, attach it to the task, follow issue workflow labels/comments, wait for required CI, and merge it yourself after review/checks pass. The user authorizes implementation, PR creation and merge. Do not bypass protection or merge failing checks.
6. Verify merge and issue closure. Report PR, merge commit, checks, review findings/fixes and any material limits. Avoid unrelated refactors or implementing other open issues.

If delegated implementation agents are used, the user prefers Sol for implementation; keep high-level design and final acceptance responsibility with the coordinating agent. No cloud task has been created by this preparation task.
