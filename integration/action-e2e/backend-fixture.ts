import { readFile } from "node:fs/promises";
import { Pool } from "pg";
import { pg, type PgClient } from "../../packages/postgres/index.mts";
import { ActionRejected, createBackend, devAuth, type Handlers, type Loaders } from "./backend.ts";

export async function createFixture() {
  const pool = new Pool({ connectionString: process.env.DATABASE_URL });
  let handlerCalls = 0;
  let loaderCalls = 0;
  const handlers: Handlers<PgClient> = {
    async addTodo({ ctx, args }) {
      handlerCalls++;
      await ctx.tx.query("INSERT INTO action_e2e_todo(id,title) VALUES($1,$2)", [args.todo.id, args.todo.title.trim()]);
      ctx.publish({ channel: "todos:demo" });
    },
    async updateTodo({ ctx, args }) {
      handlerCalls++;
      const changed = await ctx.tx.query("UPDATE action_e2e_todo SET title=$2 WHERE id=$1 RETURNING id", [args.todo.id, args.todo.title?.trim()]);
      if (changed.rows.length === 0) throw new ActionRejected("todo.missing");
      ctx.publish({ channel: "todos:demo" });
    },
    async deleteTodo({ ctx, args }) {
      handlerCalls++;
      const changed = await ctx.tx.query("DELETE FROM action_e2e_todo WHERE id=$1 RETURNING id", [args.todo.id]);
      if (changed.rows.length === 0) throw new ActionRejected("todo.missing");
      ctx.publish({ channel: "todos:demo" });
    },
    async sendEmail({ ctx, args }) {
      handlerCalls++;
      const inserted = await ctx.tx.query("INSERT INTO action_e2e_outbox(recipient,subject,body) VALUES($1,$2,$3) RETURNING id", [args.to, args.subject, args.body]);
      return { messageId: String(inserted.rows[0].id) };
    },
    async searchTodos({ ctx, args }) {
      handlerCalls++;
      if (ctx.userId !== "alice") throw Error("untrusted Action owner");
      const rows = (await ctx.tx.query("SELECT id FROM action_e2e_todo WHERE ($1::text IS NULL OR title ILIKE '%' || $1 || '%') ORDER BY id", [args.query])).rows;
      return { todos: rows.map((row) => ({ id: String(row.id) })), first: rows[0] ? { id: String(rows[0].id) } : null, count: rows.length, labels: rows.map((row) => String(row.id)), hint: args.query };
    },
  };
  const loaders: Loaders<PgClient> = {
    async todo({ ids, tx }) {
      loaderCalls++;
      const rows: ({ id: string; title: string } | null)[] = [];
      for (const { id } of ids) {
        const row = (await tx.query("SELECT id,title FROM action_e2e_todo WHERE id=$1", [id])).rows[0];
        rows.push(row ? { id: String(row.id), title: String(row.title) } : null);
      }
      return rows;
    },
  };
  const backend = createBackend<PgClient>({ database: pg(pool), authenticate: devAuth(), handlers, loaders });
  let listener: Awaited<ReturnType<typeof backend.listen>> | undefined;
  return {
    pool,
    backend,
    get handlerCalls() { return handlerCalls; },
    get loaderCalls() { return loaderCalls; },
    async initialize() {
      const migration = await readFile(new URL("../../packages/postgres/migration.sql", import.meta.url), "utf8");
      for (const sql of migration.split(";").map((statement) => statement.trim()).filter(Boolean)) await pool.query(sql);
      await pool.query("CREATE TABLE action_e2e_todo(id text PRIMARY KEY,title text NOT NULL)");
      await pool.query("CREATE TABLE action_e2e_outbox(id bigserial PRIMARY KEY,recipient text NOT NULL,subject text NOT NULL,body text NOT NULL)");
    },
    async listen() { listener = await backend.listen({ port: 0 }); return listener; },
    async close() { await listener?.close(); await pool.end(); },
  };
}
