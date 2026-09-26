# Implementation handoff: Model creation defaults (#27)

Implement [#27](https://github.com/zanminwang/axton/issues/27) from `codex/27-model-defaults`, based on `ceced50` (merged #157). The branch contains reviewed preparation documents only. On the original host, reuse `/Users/stevewang/.codex/worktrees/27-model-defaults/axton`; on another host, fetch the branch into an isolated checkout.

Read [spec](../specs/2026-09-25-27-model-defaults-design.md), [plan](2026-09-25-27-model-defaults.md), repository AGENTS and the issue workflow before implementation. The spec owns behavior; the plan owns task order and evidence. Implement the whole milestone, not only constants. If delegating implementation, use Sol as requested by the user.

## Non-negotiable decisions

- Source @default covers fixed scalar/enum values, uuid() on String/UUID and now() on DateTime.
- All source defaults trigger only for an omitted field of a fresh create: local Model CRUD and Model.create Mutation inputs. Updates, deletes, reads, sync, retries, replay and migration do not evaluate them.
- Generate concrete UUID/time once in Rust client preparation, before local persistence/network, and preserve those values across every retry/reopen.
- Separate creation metadata from the existing internal migration/read-projection default field. No source @default automatic historical backfill, including constants.
- Explicit values win; explicit nullable null stays null. Complete Model/read/handler types remain complete; only create inputs admit omission for defaults.
- Keep existing create completion/return contract. Default-only policy changes do not require backend contract version bumps or rewrite old data; structural read/input changes retain their existing rules.
- Query once (#158) and Bootstrap behavior (#151) are outside scope.

## Execution and completion

Claim the issue, follow the plan with meaningful failing tests, update architecture/guides/examples, and record material design changes in the issue. Resolve routine engineering details without additional approval. Review the final implementation yourself, fix actionable findings, run required local checks, open a PR with Closes #27, attach it to your task and satisfy required CI before merging. The user's standing instruction is to carry assigned implementation through review and merge; do not stop at code completion or PR creation. Never bypass failing checks or branch protection. Report an actual blocker rather than claiming completion.

The current user request commissioned these documents and their review only; this handoff is for a separately assigned agent, not evidence that execution has started.

## Parallel integration

#158 can proceed independently but overlaps compiler generation/validation and generated fixtures. #151 also overlaps those files and storage. Fetch main before final review; integrate whichever work merged first and preserve all behaviors. Test combined behavior where code is present. A later merger owns combined regressions for work not yet present; state that limit explicitly.

## Evidence to report

Preparation baseline: `cargo test -p axton-core -p axton-compiler -p axton-sqlite --locked`, 329 passed, 0 failed in 26 suite summaries. This is pre-feature evidence only. Report the implementation PR, merge commit, actual test commands/results (including TS/Dart and full host gate), and any remaining limits. Keep unrelated `.claude/` content untouched.
