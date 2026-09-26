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
  /**
   * Entry ids whose read fails, as `failLoads` sets them. A Loader may throw;
   * the framework then asks for each identity on its own, so exactly these
   * become `loader.failed` error records and the rest of the page still
   * resolves (guarantee D7).
   */
  const refusing = new Set<string>();
  const loaders: Loaders<Tx> = {
    async entry({ ids, tx }) {
      if (ids.some((identity) => refusing.has(identity.id)))
        throw new Error(`the Entry loader refuses ${ids.map((i) => i.id).join(", ")}`);
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
     * Publish the current `Entry` rows again, changing them: a new stamp and a
     * new position. A subscription's origin is the first head its handshake
     * acknowledges ([#150](https://github.com/zanminwang/axton/issues/150)), so
     * a client that subscribes after `initialize` meets the seeded rows either
     * this way or through `subscription.bootstrap()`
     * ([#151](https://github.com/zanminwang/axton/issues/151)); `republish`
     * below is the version that moves a position without touching the stamp.
     */
    async notify(ids: string[] = ["entry-1"], channel = "book:demo") {
      await backend.transaction(async ({ changes, publish }) => {
        for (const id of ids) changes.add({ model: "Entry", identity: { id } });
        publish({ channel });
      });
    },
    /**
     * Write `count` new `Entry` rows and publish them on `channel`, each at its
     * own cursor. Written in batches, because one interactive PostgreSQL
     * transaction per hundred records runs into the driver's transaction
     * timeout; the returned ids are in publication order.
     */
    async publishMany(
      count: number,
      options: { channel: string; prefix: string; from?: number; batch?: number },
    ): Promise<string[]> {
      const from = options.from ?? 1;
      const size = options.batch ?? 20;
      const ids = Array.from({ length: count }, (_, i) => `${options.prefix}-${from + i}`);
      for (let start = 0; start < ids.length; start += size) {
        const batch = ids.slice(start, start + size);
        await backend.transaction(async ({ tx, changes, publish }) => {
          for (const id of batch) {
            await tx.entry.upsert({
              where: { id },
              create: { id, text: `${id} text` },
              update: { text: `${id} text` },
            });
            changes.add({ model: "Entry", identity: { id } });
          }
          publish({ channel: options.channel });
        });
      }
      return ids;
    },
    /** Write one `Entry` and publish it on every named channel. */
    async publishOne(id: string, text: string, channels: string[]): Promise<void> {
      await backend.transaction(async ({ tx, changes, publish }) => {
        await tx.entry.upsert({ where: { id }, create: { id, text }, update: { text } });
        changes.add({ model: "Entry", identity: { id } });
        for (const channel of channels) publish({ channel });
      });
    },
    /**
     * Publish existing records again on `channel` without changing them: each
     * takes a new cursor at the stamp it already has (guarantee D3). This is
     * how a record leaves a subscription's historical interval and becomes the
     * subscription's own delivery ([#151](https://github.com/zanminwang/axton/issues/151)).
     */
    async republish(ids: string[], channel: string): Promise<void> {
      await backend.transaction(async ({ publish }) => {
        publish({
          channel,
          records: ids.map((id) => ({ model: "Entry", identity: { id } })),
        });
      });
    },
    /** Delete the row and publish it: the Loader answers `null`, an authoritative deletion (D6). */
    async tombstone(id: string, channel: string): Promise<void> {
      await backend.transaction(async ({ tx, changes, publish }) => {
        await tx.entry.delete({ where: { id } });
        changes.add({ model: "Entry", identity: { id } });
        publish({ channel });
      });
    },
    /** Make the `Entry` Loader fail for these ids until `allowLoads` clears them. */
    failLoads(...ids: string[]) {
      for (const id of ids) refusing.add(id);
    },
    allowLoads(...ids: string[]) {
      for (const id of ids) refusing.delete(id);
    },
    /** The channel head: the highest cursor the invalidation log has allocated. */
    async head(channel: string): Promise<number> {
      const rows = await db.$queryRawUnsafe<{ head: bigint }[]>(
        "SELECT head FROM axton_channel WHERE channel = $1",
        channel,
      );
      return rows.length === 0 ? 0 : Number(rows[0]!.head);
    },
    /** The one retained position `id` has on `channel`, or `null`; a republication replaces it in place. */
    async positionOf(channel: string, id: string): Promise<number | null> {
      const rows = await db.$queryRawUnsafe<{ cursor: bigint }[]>(
        'SELECT cursor FROM axton_invalidation WHERE channel = $1 AND model = \'Entry\' AND identity_key = $2',
        channel,
        JSON.stringify({ id }),
      );
      return rows.length === 0 ? null : Number(rows[0]!.cursor);
    },
    /**
     * Empty every AXTON table and the `Entry` rows. A bootstrap scenario asserts
     * cursors and channel heads, so it starts from an empty log rather than from
     * whatever an earlier scenario in the same database left behind.
     */
    async reset(): Promise<void> {
      await db.$executeRawUnsafe(
        "TRUNCATE axton_membership, axton_invalidation, axton_channel, axton_record, axton_client, axton_call",
      );
      await db.$executeRawUnsafe('DELETE FROM "Entry"');
      refusing.clear();
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
