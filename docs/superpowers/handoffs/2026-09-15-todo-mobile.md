# Handoff: mobile To-do demo (#31)

## Status 2026-09-15

Implemented on branch `codex/todo-mobile` (worktree `.worktrees/todo-mobile`), stacked on `codex/react-native` (#100). The To-do backend and its 15 real-backend e2e scenarios pass (`bash integration/e2e/todo-run.sh`), the Expo app builds with embedded JavaScript, and `bash integration/platform/run_todo_ios_smoke.sh` passed on two iOS 26.5 simulators: live add/done both ways, one dropped push response retried without a duplicate handler run, offline add-then-done, process termination and relaunch with the same client identity, Bob's independent add, reconnect and convergence of both phones with PostgreSQL. `examples/rust-round-trip` moved to `integration/e2e/fixtures/round-trip`; the website quickstart points at `examples/todo`. Evidence and limits: `examples/todo/README.md`. Neither branch is pushed or merged.


## Copyable agent prompt

**Scope correction:** Work is split into two issues. [React Native support #100](https://github.com/zanminwang/axton/issues/100) blocks [To-do demo #31](https://github.com/zanminwang/axton/issues/31). Complete #100 using `docs/superpowers/plans/2026-09-15-react-native-support.md` first. If already working from the original handoff, preserve current changes and separate SDK work under #100; do not continue treating both as one demo issue.

After #100 supplies its verified integration, implement #31 using these documents, in order:

1. `docs/superpowers/specs/2026-09-15-todo-mobile-design.md`
2. `docs/superpowers/plans/2026-09-15-todo-mobile.md`

Reuse this isolated worktree on this machine:

```text
/Users/stevewang/Github/local first state/.worktrees/todo-mobile
branch: codex/todo-mobile
planning baseline: a837cf8
```

Read `AGENTS.md` and inspect branch/status first. Preserve the planning documents and unrelated changes; commit the documents before updating the implementation baseline. If working on another machine, obtain this branch's planning documents first and create an isolated `codex/` worktree. Do not assume locally saved files are already pushed to GitHub.

The user has settled the product scope: **React Native + TypeScript, Add task and Done only**. Two independent phones use Alice and Bob identities with simple initial avatars and show the same shared task list. Exactly two business tables: `User(id, name)` and `Todo(id, title, done, createdById)`. Creator is not assignee. There are no replies, assignment, edit/delete controls, multiple lists, or extra dashboard UI. The app renders one phone screen; the marketing demonstration shows two app instances side by side.

Use the existing Rust/SQLite engine, generated clients and TypeScript/Prisma/PostgreSQL backend. Consume the React Native native carrier and client integration delivered by #100; do not implement them inside #31. Use an Expo development build; iOS is the first required target. Preserve Node transaction/savepoint behavior and existing Dart coverage. Check whether #58's Rust live-session migration has landed; use current implemented APIs, not a planned command family.

**Implementation simplicity:** #100 provides the necessary React Native adapter; #31 consumes it. Reuse existing code and extract shared helpers only for concrete needs. A generic client factory, broad SDK refactor, or complete Node raw-API parity is not required. Preserve transaction/offline/sync correctness and their verification. This clarification supersedes the earlier plan's mandatory shared-runtime extraction.

Work through the plan and verify each deliverable. The decisive mobile test uses two separate simulator installations/databases/client IDs, actual backend synchronization, offline add-then-done, app termination/relaunch while disconnected, and convergence after reconnect. Build with embedded JavaScript for the offline-relaunch test. A mockup, SDK pause alone, Node-only tests, or native link success does not complete that test.

Replace `examples/rust-round-trip` with `examples/todo` only after moving its useful normalization/rejection/retry/catch-up and JS/Dart regressions into an integration fixture and updating scripts/docs. Do not delete tests to make the smaller demo pass.

Proceed with implementation within this scope. Keep user updates concise. Finish independent work if a platform check is blocked, then report the exact missing evidence. Do not expand into browser/WASM, Android, production authentication, deployment, or video production. Do not merge or publish automatically.

## State at preparation

- #100 owns React Native support and formally blocks #31.
- #31 exists and owns the mobile demo; its previous Flutter/assignment/CRUD description is superseded by this spec.
- #59 owns browser/WASM/runtime/storage/transport dependencies.
- #72 is the web version of this same demo. #31 and #59 block #72; mobile is not blocked by browser work.
- The demo worktree was rebased to the then-current local main, `a837cf8`.
- This handoff adds design and planning documents only. It does not implement the app or native adapter.
- Earlier HTML previews simulated shared state. They are not runtime evidence or a source of additional features; use the spec's minimal wireframe.
- No implementation agent has been launched by this planning task.
- Planning validation passed: the spec's exact schema compiled with the repository compiler at `a837cf8`; generated AddTodo/SetTodoDone builders produced the expected operation shapes; local Markdown links and fenced blocks checked cleanly. These checks do not validate mobile execution or backend synchronization.

## Expected final report

Include the resulting app/backend locations, exact two-simulator launch steps, verified revision/toolchain, executed tests and runtime screenshots, preserved regression coverage, and material remaining limitations. Map results to acceptance criteria A1–A9 in the spec. Keep #31 open if required mobile runtime evidence is missing.
