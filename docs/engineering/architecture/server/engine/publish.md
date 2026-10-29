# Publish

## 1. Introduction and Goals

Explicit track creates interest with a real discoverable position; repeat live tracking retains its position. Global invalidation reaches current holders and targeted invalidation reaches selected existing holders, without enrolling new ones. Record guards and canonical Stream locks keep content and position consistent. Each publication scope reserves at most one cursor per affected Stream; grouped Mutations use separate scopes inside their shared transaction. Wakes occur after commit. See [commit groups](../protocol5.md#sequential-commit-groups).

## 5. Building Block View

[settlement.rs](../../../../../crates/server/src/settlement.rs) normalizes tracking/invalidation, locks Streams and applies final membership through the Host. The [SDK effects collector](../../../../../packages/backend/server/bindings/effects.mts) records declarations; [PostgreSQL persistence](../../../../../packages/backend/postgres/src/persistence.mts) answers the retained Host requests. [Protocol 5](../../protocols/sync.md) owns shared context, delivery and settlement rules.
