---
hide:
  - navigation
  - toc
---

# AXTON

AXTON is a schema-driven framework for building local-first apps with your own backend.

Define Models, Mutations and Queries once. AXTON generates typed client calls and backend interfaces, keeps local state in SQLite, and synchronizes through your handlers and Loaders.

- **Schema-driven.** Describe local Models and backend Mutations and Queries in one contract.
- **Typed end to end.** Generate client APIs and backend read/write interfaces together.
- **Works offline.** Read and write locally; pending changes persist until they can sync.
- **Your backend.** Keep your business logic and database. No vendor cloud service required.

## Start here

[Run the getting-started tutorial](getting-started.md) to run the collaborative To-do example on two simulators, work offline and see your backend accept or reject a Mutation.

| What you need | Read |
| --- | --- |
| Define Models, Mutations and Queries | [Schema guide](schema/define.md) |
| Use the client | [Client setup](frontend/setup.md) |
| Implement handlers and loaders | [Backend guide](backend/setup.md) |
| Understand local state and sync | [Concepts](concepts.md) |
| Look up a method, type or option | [API reference](api-index.md) |
| Handle offline work and failures | [Sync and recovery](frontend/sync.md) |

## Supported integrations

| Layer | Available integration |
| --- | --- |
| Client | TypeScript · Flutter |
| Backend | TypeScript |
| Backend database | [PostgreSQL](backend/database.md) through `pg`, Prisma or Drizzle; a two-method driver for other tools |

The TypeScript client and backend currently run on Node.js. Clients use native Rust bindings; browser support is not implemented. See [platform validation](frontend/platforms.md) for tested environments and mobile setup.

Packages are currently used from source. Follow the tutorial's repository commands rather than installing an unpublished package. Need another language, runtime or database adapter? [Request support](https://github.com/zanminwang/axton/issues/new).

## Project

[GitHub](https://github.com/zanminwang/axton) · [Framework comparison](https://github.com/zanminwang/axton/blob/main/README.md#how-axton-compares-with-other-sync-frameworks) · [Issues](https://github.com/zanminwang/axton/issues) · [Contribute to the documentation](https://github.com/zanminwang/axton/blob/main/docs/writing/guides.md)
