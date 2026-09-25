import { PrismaClient, type Prisma } from "@prisma/client";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { prisma } from "../../../../packages/postgres/index.mts";
import {
  createBackend,
  devAuth,
  MutationRejected,
  type Handlers,
  type Loaders,
} from "./generated/backend.ts";
import { schema } from "./generated/generated.ts";

type Tx = Prisma.TransactionClient;

export async function createExample() {
  const db = new PrismaClient();
  let calls = 0;
  const handlers: Handlers<Tx> = {
    async edit({ input, tx, publish }) {
      calls++;
      const { identity, patch } = input.entry;
      if (patch.text === "reject") throw new MutationRejected("entry.denied");
      await tx.entry.update({
        where: identity,
        data: { ...patch, ...(typeof patch.text === "string" ? { text: patch.text.trim() } : {}) },
      });
      // The edited entry is stamped and read back for the receipt regardless;
      // publishing distributes that same version to the channel's subscribers.
      publish({ channel: "book:demo" });
    },
  };
  const loaders: Loaders<Tx> = {
    async entry({ ids, tx }) {
      return Promise.all(ids.map((identity) => tx.entry.findUnique({ where: identity })));
    },
  };
  const backend = createBackend<Tx>({
    database: prisma(db),
    authenticate: devAuth(),
    handlers,
    loaders,
  });
  let server: Awaited<ReturnType<typeof backend.listen>> | undefined;
  return {
    db,
    backend,
    schema,
    get handlerCalls() {
      return calls;
    },
    async initialize() {
      const migration = await readFile(
        new URL("../../../../packages/postgres/migration.sql", import.meta.url),
        "utf8",
      );
      for (const sql of migration.split(";").map((s) => s.trim()).filter(Boolean))
        await db.$executeRawUnsafe(sql);
      await db.$executeRawUnsafe(
        'CREATE TABLE IF NOT EXISTS "Entry" (id TEXT PRIMARY KEY,text TEXT NOT NULL,note TEXT)',
      );
      await backend.transaction(async ({ tx, changes, publish }) => {
        await tx.entry.upsert({
          where: { id: "entry-1" },
          create: { id: "entry-1", text: "Hello from the server" },
          update: {},
        });
        changes.add({ model: "Entry", identity: { id: "entry-1" } });
        publish({ channel: "book:demo" });
      });
    },
    /**
     * Publish the current `Entry` rows again, without changing them. A
     * subscription's origin is the first head its handshake acknowledges
     * ([#150](https://github.com/zanminwang/axton/issues/150)), so a client that
     * subscribes after `initialize` receives the seeded rows only when they are
     * published again. Whole-Scope loading is #151's `bootstrap()`.
     */
    async notify(ids: string[] = ["entry-1"]) {
      await backend.transaction(async ({ changes, publish }) => {
        for (const id of ids) changes.add({ model: "Entry", identity: { id } });
        publish({ channel: "book:demo" });
      });
    },
    listen(port: number) {
      return backend.listen({ port }).then((started) => {
        server = started;
        return started;
      });
    },
    async close() {
      await server?.close();
      await db.$disconnect();
    },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const app = await createExample();
  await app.initialize();
  const server = await app.listen(Number(process.env.PORT ?? 4242));
  console.log(`Example listening at ${server.url}`);
  for (const signal of ["SIGINT", "SIGTERM"] as const)
    process.once(signal, () => void server.close().then(() => app.close()));
}
