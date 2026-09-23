# Local-only public transactions — design (#145)

Status: approved issue design, implementation pending. This document records the target contract for [#145](https://github.com/zanminwang/axton/issues/145), the prerequisite to #141.

## Context and boundary

Today `GeneratedTransaction` in `crates/compiler/src/emit.rs` exposes `mutate`, and `WritePort` includes `MutatePort`. Node, React Native and Dart raw transaction objects also expose `mutate`; standalone `client.mutate` delegates through that method. The Rust/SQLite engine already owns atomic optimism and enqueueing. The change is at the public SDK boundary, not a removal of the engine's enqueue, savepoint or receipt logic.

## Public contract

`client.transaction(async tx => { ... })` provides transactional Model reads and direct `create`, `update` and `delete`. Its writes commit together or roll back together; reads see prior writes in the callback. Node and Dart retain supported savepoints and all platforms retain transaction lifetime and unawaited-operation checks. `tx.mutate`, `tx.actions`, any equivalent raw transaction enqueue method, and transaction watch are absent. Watch remains a live-client operation and observes committed changes.

Standalone `client.mutate.<name>(args)` retains its spelling and ordinal result until #141 changes the API. The runtime performs its optimistic writes and queue insertion in one framework-owned SQLite transaction. A failed submission leaves neither partial local optimism nor a partial queue record. Offline reopen, acceptance, rejection and pending-edit replay retain their existing engine behavior. The generated `Mutate` builder therefore accepts a client-level `MutatePort`; `WritePort` contains reads and direct writes only.

The raw SDK needs a private/internal atomic enqueue helper used by `Client.mutate`, rather than a public transaction method. The transport can still send the engine's `enqueue` command while an internal transaction is active. Public transaction objects must not expose that helper through exported types or runtime properties. The public client method checks whether it is called in its own active transaction callback before queuing work, so a captured `client.mutate` fails promptly instead of deadlocking on the serialized client. Node can use its existing async context and Dart a Zone token to distinguish callback calls from unrelated concurrent work. React Native shares the client runtime but lacks Node async context. Its adapter uses a conservative fail-fast rule for client mutation calls during any active public transaction. This may reject an unrelated concurrent mutation that previously serialized; that limitation must be documented and tested.

## Migration and compatibility

Local Model-only transaction callbacks remain valid. A single `tx.mutate` becomes a standalone `client.mutate` call outside the callback. Several independent mutations can run separately; that does not preserve their former shared local atomicity. To preserve one business operation, declare one named mutation with the required inputs. Review mixed direct writes and mutations individually rather than moving a call across the transaction boundary and asserting equivalent behavior. No Action schema or naming changes occur here.

## Responsibilities and evidence

The compiler changes generated TypeScript/Dart ports and facades; `packages/client-js`, `packages/client-react-native` and `packages/dart/lib/src/client.dart` change raw SDK boundaries and standalone orchestration. Generated fixtures and examples must be regenerated and rewritten to assert the same intended behavior without `tx.mutate`. Update the owning typed API architecture and website client API/index. Tests must cover negative generated API checks, direct-write rollback/read-your-writes/savepoints/lifetime, internal atomic enqueue including failed submission and reopen, receipt acceptance/rejection, and a captured-client call inside a callback that rejects promptly.

This design preserves guarantees L3, P6 and R3 in `docs/engineering/guarantees.md`; close/reopen alone is evidence for reopen durability, not arbitrary crash-boundary recovery. #141 owns the later Model/Action spelling, while #142 owns new Action execution.
