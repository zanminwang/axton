# @axtonjs/postgres

> **Alpha.** AXTON is alpha software: its API is unstable and it is not ready for production use.

AXTON's PostgreSQL persistence: the framework tables (`@axtonjs/postgres/migration.sql`), every statement AXTON runs, and a two-method driver interface. Pick the shim for your access tool and pass it as `createBackend({ database })`:

```ts
import { pg } from "@axtonjs/postgres/pg";                // node-postgres pool
import { prisma } from "@axtonjs/postgres/prisma";        // Prisma client
import { drizzle } from "@axtonjs/postgres/drizzle";      // drizzle-orm/node-postgres
```

`pg` and `prisma` are also exported from `@axtonjs/postgres`; `drizzle` is not, because it imports `drizzle-orm`. Any other tool needs `persistence({ transaction, query })`. See the [database guide](https://github.com/zanminwang/axton/blob/main/website/docs/backend/database.md).

## Schema

`migration.sql` installs five canonical tables in a fresh namespace: `axton_stream`, `axton_record`, `axton_store`, `axton_mutation_result`, and `axton_stream_record`. Apply the whole file transactionally. Per-transaction cursor reservation is ephemeral SQL state, not another durable table.

Global publication fencing, its host operation and public helper are removed.
Reapplying the migration drops the unused `axton_publication_fence` table from a
current protocol-5 namespace. Incompatible historical layouts are refused before
any writes. Serializable transactions, record/Stream guards and whole-transaction
retries remain required.

Initialized read requests inspect Store ownership with a snapshot SELECT. Progress
writers retain `FOR NO KEY UPDATE` on progress. First claims and Bootstrap
initialization remain protected. Delivery reads current cursor pages without
saving response payloads or running shared expiry cleanup. The migration drops
the retired `axton_delivery_unit` and `axton_delivery_plan` tables; business data,
Store progress, outcomes and Stream facts remain.

An installed legacy framework layout is refused without changing its data. This candidate supplies no old-namespace migration, local-file upgrade or compatibility bridge. Use fresh framework storage and review [protocol-5 adoption](https://github.com/zanminwang/axton/blob/main/docs/engineering/protocol5-adoption.md) before replacing an existing deployment.
