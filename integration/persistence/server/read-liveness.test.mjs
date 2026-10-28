// A suspended ordinary Query must not serialize independent Batch progress.
import test, { after } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { createHash } from "node:crypto";
import { Pool } from "pg";
import { createBackend } from "../../../packages/backend/server/index.mts";
import { pg } from "../../../packages/backend/postgres/index.mts";
const native = createRequire(import.meta.url)(
  "../../../bindings/node/axton-node.node",
);
const pool = new Pool({ connectionString: process.env.DATABASE_URL });
after(() => pool.end());
const canonical = (v) =>
  Array.isArray(v)
    ? "[" + v.map(canonical).join(",") + "]"
    : v && typeof v === "object"
      ? "{" +
        Object.keys(v)
          .sort()
          .map((k) => JSON.stringify(k) + ":" + canonical(v[k]))
          .join(",") +
        "}"
      : JSON.stringify(v);
const hash = (domain, v) =>
  createHash("sha256")
    .update(domain + "\0")
    .update(canonical(v))
    .digest("hex");
const read = {
    name: "Read",
    version: 1,
    kind: "query",
    inputs: [],
    outputs: [],
  },
  write = { ...read, name: "Write", kind: "mutation" };
const config = {
  schema: {
    enums: [],
    models: [
      {
        name: "Task",
        version: 1,
        identity: ["id"],
        fields: ["id", "title"].map((name) => ({
          name,
          type: { kind: "scalar", name: "string" },
          nullable: false,
        })),
      },
    ],
    actions: [read, write],
  },
  loaders: ["Task"],
};
for (const kind of ["Query", "Fetch", "Query Model Loader"])
  for (const store of kind === "Query Model Loader" ? [false, true] : [false])
    for (const existing of [false, true])
      test(
        `suspended ${kind} admits same-Store Mutation on ${existing ? "existing" : "new"} Store (store:${store})`,
        { timeout: 10000 },
        async () => {
          const namespace = `read_liveness_${kind.replaceAll(" ", "_")}_${existing}_${store}`,
            admin = await pool.connect();
          let release, entered;
          const gate = new Promise((r) => (release = r)),
            arrived = new Promise((r) => (entered = r));
          const base = pg(pool),
            database = {
              ...base,
              transaction: (body) =>
                base.transaction(async (tx) => {
                  await tx.query(`SET LOCAL search_path=${namespace}`);
                  return body(tx);
                }),
            };
          let pending, mutation;
          try {
            await admin.query(`CREATE SCHEMA ${namespace}`);
            await admin.query(`SET search_path=${namespace}`);
            await admin.query(
              await readFile(
                new URL(
                  "../../../packages/backend/postgres/migration.sql",
                  import.meta.url,
                ),
                "utf8",
              ),
            );
            await admin.query(
              "CREATE TABLE business(id text PRIMARY KEY,title text NOT NULL)",
            );
            if (kind === "Query Model Loader")
              await admin.query(
                "INSERT INTO business VALUES('initial','before')",
              );
            const cfg =
              kind === "Query Model Loader"
                ? {
                    ...config,
                    schema: {
                      ...config.schema,
                      actions: [
                        {
                          ...read,
                          outputs: [
                            {
                              name: "task",
                              kind: "model",
                              model: "Task",
                              modelReadVersion: 1,
                              cardinality: "single",
                              source: "handlerIdentity",
                              handlerType: {
                                kind: "identity",
                                model: "Task",
                                fields: [
                                  {
                                    name: "id",
                                    type: { kind: "scalar", name: "string" },
                                  },
                                ],
                              },
                            },
                          ],
                        },
                        write,
                      ],
                      resultModels: config.schema.models.map((model) => ({
                        ...model,
                        enums: [],
                      })),
                    },
                  }
                : config;
            const app = createBackend({
              config: cfg,
              native,
              database,
              protocol5: {
                authorizeStream: (owner, stream) => stream === `User:${owner}`,
              },
              authenticate: () => "alice",
              loaders: {
                task: async ({ tx, ids }) => {
                  entered();
                  await gate;
                  return Promise.all(
                    ids.map(
                      async ({ id }) =>
                        (
                          await tx.query(
                            "SELECT id,title FROM business WHERE id=$1",
                            [id],
                          )
                        ).rows[0] ?? null,
                    ),
                  );
                },
              },
              queries: {
                read: async () => {
                  if (kind === "Query Model Loader")
                    return { task: { id: "initial" } };
                  entered();
                  await gate;
                  return {};
                },
              },
              mutations: {
                write: async ({ ctx }) => {
                  await ctx.tx.query(
                    "INSERT INTO business VALUES('committed','ok')",
                  );
                  return {};
                },
              },
            });
            const context = {
              protocol: 5,
              storeId: "same-store",
              stream: "User:alice",
              materialization: app.materializationId,
            };
            if (existing)
              await app.handshake(
                "alice",
                JSON.stringify({
                  protocol: 5,
                  storeId: context.storeId,
                  stream: context.stream,
                }),
              );
            pending = app.action(
              "alice",
              JSON.stringify({
                ...context,
                requestId: "suspended",
                store,
                invocation:
                  kind !== "Fetch"
                    ? { kind: "query", name: "Read", version: 1, args: {} }
                    : {
                        kind: "fetch",
                        key: { model: "Task", identity: { id: "committed" } },
                        version: 1,
                      },
              }),
            );
            pending.catch(() => {});
            await arrived;
            const request = {
              ...context,
              batchId: 1,
              digest: "",
              mutations: [
                {
                  id: 1,
                  name: "Write",
                  version: 1,
                  descriptor: hash("axton:mutation-descriptor:5", {
                    ...write,
                    input: null,
                    outputEnums: [],
                  }),
                  operations: [],
                },
              ],
            };
            request.digest = hash(
              "axton:mutation-batch:5",
              Object.fromEntries(
                Object.entries(request).filter(([key]) => key !== "digest"),
              ),
            );
            mutation = app.push("alice", JSON.stringify(request));
            mutation.catch(() => {});
            let timer;
            try {
              const ack = JSON.parse(
                await Promise.race([
                  mutation,
                  new Promise(
                    (_, reject) =>
                      (timer = setTimeout(
                        () =>
                          reject(
                            new Error("Mutation blocked by suspended Query"),
                          ),
                        1500,
                      )),
                  ),
                ]),
              );
              assert.equal(ack.results[0].outcome.kind, "accepted");
            } finally {
              clearTimeout(timer);
            }
            assert.deepEqual(
              (
                await admin.query(
                  "SELECT id FROM business WHERE id='committed'",
                )
              ).rows,
              [{ id: "committed" }],
            );
            release();
            assert.equal(JSON.parse(await pending).outcome.kind, "succeeded");
            assert.equal(
              (
                await admin.query(
                  "SELECT last_processed_batch_id FROM axton_store WHERE id='same-store'",
                )
              ).rows[0].last_processed_batch_id,
              "1",
            );
          } finally {
            release();
            await Promise.allSettled([pending, mutation]);
            await admin.query(`DROP SCHEMA ${namespace} CASCADE`);
            admin.release();
          }
        },
      );

test("ordinary read refuses existing Store owner or Stream mismatch before application work", async () => {
  const namespace = "read_binding_refusal",
    admin = await pool.connect(),
    base = pg(pool);
  const database = {
    ...base,
    transaction: (body) =>
      base.transaction(async (tx) => {
        await tx.query(`SET LOCAL search_path=${namespace}`);
        return body(tx);
      }),
  };
  let handled = 0;
  try {
    await admin.query(`CREATE SCHEMA ${namespace}`);
    await admin.query(`SET search_path=${namespace}`);
    await admin.query(
      await readFile(
        new URL(
          "../../../packages/backend/postgres/migration.sql",
          import.meta.url,
        ),
        "utf8",
      ),
    );
    const app = createBackend({
      config,
      native,
      database,
      authenticate: () => "alice",
      protocol5: {
        authorizeStream: (owner, stream) => stream === `User:${owner}`,
      },
      mutations: { write: async () => ({}) },
      queries: {
        read: async () => {
          handled++;
          return {};
        },
      },
      loaders: { task: async ({ ids }) => ids.map(() => null) },
    });
    await app.handshake(
      "alice",
      JSON.stringify({ protocol: 5, storeId: "owned", stream: "User:alice" }),
    );
    for (const [owner, stream] of [
      ["bob", "User:bob"],
      ["alice", "User:other"],
    ])
      await assert.rejects(
        app.action(
          owner,
          JSON.stringify({
            protocol: 5,
            storeId: "owned",
            stream,
            materialization: app.materializationId,
            requestId: "forged",
            store: false,
            invocation: { kind: "query", name: "Read", version: 1, args: {} },
          }),
        ),
        (error) => error.code === "store.binding",
      );
    assert.equal(handled, 0);
    assert.deepEqual(
      (
        await admin.query(
          "SELECT principal,stream,last_processed_batch_id FROM axton_store WHERE id='owned'",
        )
      ).rows,
      [
        {
          principal: "alice",
          stream: "User:alice",
          last_processed_batch_id: "0",
        },
      ],
    );
  } finally {
    await admin.query(`DROP SCHEMA ${namespace} CASCADE`);
    admin.release();
  }
});

// Exercise complete cursor reads in READ ONLY transactions,
// while a real same-Store Mutation holds its progress guard inside a handler.
for (const kind of [
  "handshake",
  "pull",
  "live",
  "materialization",
  "cursor_resume",
]) {
  test(
    `${kind} on an initialized Store completes while its Mutation handler is suspended`,
    { timeout: 15000 },
    async () => {
      const namespace = `delivery_liveness_${kind}`,
        admin = await pool.connect(),
        base = pg(pool);
      let readOnly = false;
      const database = {
        ...base,
        transaction: (body) =>
          base.transaction(async (tx) => {
            if (readOnly) await tx.query("SET TRANSACTION READ ONLY");
            await tx.query(`SET LOCAL search_path=${namespace}`);
            return body(tx);
          }),
      };
      let entered,
        release,
        timer,
        pending,
        duplicate,
        reading,
        allow = true,
        loads = 0;
      const arrived = new Promise((resolve) => (entered = resolve)),
        gate = new Promise((resolve) => (release = resolve));
      try {
        await admin.query(`CREATE SCHEMA ${namespace}`);
        await admin.query(`SET search_path=${namespace}`);
        await admin.query(
          await readFile(
            new URL(
              "../../../packages/backend/postgres/migration.sql",
              import.meta.url,
            ),
            "utf8",
          ),
        );
        await admin.query(
          "CREATE TABLE business(id text PRIMARY KEY,title text NOT NULL); INSERT INTO business VALUES('initial','before')",
        );
        const cfg = structuredClone(config);
        cfg.schema.models[0].bootstrap = true;
        const previousMaterialization = native.serverMaterializationId05(
          JSON.stringify(cfg),
          "previous",
        );
        const app = createBackend({
          config: cfg,
          native,
          database,
          authenticate: () => "alice",
          protocol5: {
            materializations: {
              [previousMaterialization]: {
                schema: cfg.schema,
                projectionGeneration: "previous",
              },
            },
            authorizeStream: (owner, stream) =>
              allow && stream === `User:${owner}`,
          },
          queries: { read: async () => ({}) },
          loaders: {
            task: async ({ tx, ids }) => {
              loads++;
              return Promise.all(
                ids.map(
                  async ({ id }) =>
                    (
                      await tx.query(
                        "SELECT id,title FROM business WHERE id=$1",
                        [id],
                      )
                    ).rows[0] ?? null,
                ),
              );
            },
          },
          mutations: {
            write: async ({ ctx }) => {
              entered();
              await gate;
              await ctx.tx.query(
                "INSERT INTO business VALUES('committed','ok')",
              );
              return {};
            },
          },
        });
        await app.transaction(({ streams }) =>
          streams(["User:alice"]).track.task({ id: "initial" }),
        );
        if (kind === "cursor_resume") {
          await admin.query("INSERT INTO business VALUES('second','before')");
          await app.transaction(({ streams }) =>
            streams(["User:alice"]).track.task({ id: "second" }),
          );
        }
        const hello = {
          protocol: 5,
          storeId: "same-store",
          stream: "User:alice",
        };
        const ack = JSON.parse(
          await app.handshake("alice", JSON.stringify(hello)),
        );
        const context = { ...hello, materialization: app.materializationId };
        const delta = {
          ...context,
          after: 0,
          through: ack.head,
          bootstrap: false,
        };
        if (kind === "cursor_resume") {
          const first = JSON.parse(
            await app.pull("alice", JSON.stringify({ ...delta, through: 1 })),
          );
          delta.after = first.header.through;
          assert.equal(delta.after, 1);
        }
        const invoke = (owner = "alice", stream = "User:alice") => {
          const ctx = { ...context, stream };
          if (kind === "handshake")
            return app.handshake(owner, JSON.stringify({ ...hello, stream }));
          if (kind === "materialization")
            return app.materialize(
              owner,
              JSON.stringify({
                ...ctx,
                requestId: "owned",
                owner: {
                  kind: "schema",
                  previousMaterialization,
                },
                keys: [{ model: "Task", identity: { id: "initial" } }],
                models: {},
              }),
            );
          const request = {
            ...delta,
            ...ctx,
          };
          return kind === "live"
            ? app.pull05Live(owner, JSON.stringify(request))
            : app.pull(owner, JSON.stringify(request));
        };
        const beforeLoads = loads;
        for (const [owner, stream] of [
          ["bob", "User:bob"],
          ["alice", "User:other"],
        ])
          await assert.rejects(
            invoke(owner, stream),
            (error) => error.code === "store.binding",
          );
        allow = false;
        await assert.rejects(
          invoke(),
          (error) => error.code === "stream.forbidden",
        );
        allow = true;
        assert.equal(
          loads,
          beforeLoads,
          "refused bindings never invoke a Loader",
        );
        const request = {
          ...context,
          batchId: 1,
          digest: "",
          mutations: [
            {
              id: 1,
              name: "Write",
              version: 1,
              descriptor: hash("axton:mutation-descriptor:5", {
                ...write,
                input: null,
                outputEnums: [],
              }),
              operations: [],
            },
          ],
        };
        request.digest = hash(
          "axton:mutation-batch:5",
          Object.fromEntries(
            Object.entries(request).filter(([key]) => key !== "digest"),
          ),
        );
        pending = app.push("alice", JSON.stringify(request));
        pending.catch(() => {});
        await Promise.race([
          arrived,
          pending.then(() => {
            throw new Error(
              "Mutation completed before entering its suspended handler",
            );
          }),
          new Promise((_, reject) => {
            timer = setTimeout(
              () => reject(new Error("Mutation did not enter its handler")),
              5000,
            );
          }),
        ]);
        clearTimeout(timer);
        readOnly = true;
        reading = invoke();
        reading.catch(() => {});
        const response = JSON.parse(
          await Promise.race([
            reading,
            new Promise((_, reject) => {
              timer = setTimeout(
                () =>
                  reject(new Error(`${kind} blocked behind Mutation progress`)),
                5000,
              );
            }),
          ]),
        );
        clearTimeout(timer);
        readOnly = false;
        if (kind === "handshake") assert.equal(response.head, ack.head);
        else {
          const delivery = response.delivery ?? response;
          assert.equal(delivery.header.storeId, "same-store");
          const row = delivery.parts
            .flatMap((p) => p.changes)
            .find(
              (c) =>
                c.key?.identity?.id ===
                (kind === "cursor_resume" ? "second" : "initial"),
            );
          assert.equal(
            row?.state?.title,
            "before",
            "delivery sees the committed snapshot during the paused write",
          );
        }
        assert.equal(
          (
            await admin.query(
              "SELECT last_processed_batch_id FROM axton_store WHERE id='same-store'",
            )
          ).rows[0].last_processed_batch_id,
          "0",
        );
        // An identical concurrent upload must still wait for the immutable result;
        // allowing delivery must not remove Mutation-to-Mutation serialization.
        let duplicateDone = false;
        duplicate = app.push("alice", JSON.stringify(request)).then((value) => {
          duplicateDone = true;
          return value;
        });
        duplicate.catch(() => {});
        let blocked = false;
        for (let i = 0; i < 400; i++) {
          if (
            (
              await admin.query(
                "SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>$1 AND wait_event_type='Lock' AND query LIKE '%axton_store%'",
                [admin.processID],
              )
            ).rowCount
          ) {
            blocked = true;
            break;
          }
          await new Promise((resolve) => setTimeout(resolve, 5));
        }
        assert.equal(
          blocked,
          true,
          "duplicate upload reached the Store progress guard",
        );
        assert.equal(duplicateDone, false);
        release();
        const [first, again] = await Promise.all([pending, duplicate]);
        assert.deepEqual(
          JSON.parse(first),
          JSON.parse(again),
          "replay returns the one saved result",
        );
        assert.equal(JSON.parse(first).results[0].outcome.kind, "accepted");
        assert.equal(
          (
            await admin.query(
              "SELECT count(*) n FROM business WHERE id='committed'",
            )
          ).rows[0].n,
          "1",
        );
        assert.equal(
          (
            await admin.query(
              "SELECT last_processed_batch_id FROM axton_store WHERE id='same-store'",
            )
          ).rows[0].last_processed_batch_id,
          "1",
        );
      } finally {
        readOnly = false;
        clearTimeout(timer);
        release();
        await Promise.allSettled([pending, duplicate, reading]);
        await admin.query(`DROP SCHEMA ${namespace} CASCADE`);
        admin.release();
      }
    },
  );
}
