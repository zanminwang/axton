import assert from "node:assert/strict";
import test from "node:test";
import {
  ActionRejected,
  Moment,
  createBackend,
  type ActionHandlerCall,
  type Handlers,
  type Loaders,
  type PutV1Input,
} from "./backend.ts";
import type { Todo } from "./generated.ts";

test("generated backend decodes Date values and keeps canonical Model references", async () => {
  const first = "2026-01-01T00:00:00.000Z";
  const second = "2026-01-02T00:00:00.000Z";
  const seen: unknown[] = [];
  type Tx = { rows: Map<string, Todo> };
  const put = async ({ ctx, args }: ActionHandlerCall<Tx, PutV1Input>) => {
    assert.equal(args.when.getUTCFullYear(), 2026);
    assert.equal(args.todo.at.getUTCFullYear(), 2026);
    assert.deepEqual(args.statuses, ["open", "closed"]);
    assert.equal(args.note, null);
    ctx.tx.rows.set(args.todo.id, args.todo);
    ctx.changes.add(args.todo);
    ctx.changes.add(Moment({ at: new Date(first) }));
    ctx.changes.add(Moment({ at: new Date(second) }));
    ctx.changes.add({
      model: "Moment",
      identity: { at: new Date("2026-01-03T00:00:00.000Z") },
    });
    ctx.changes.add({
      model: "Moment",
      identity: { at: new Date("2026-01-04T00:00:00.000Z") },
    });
    ctx.publish({ channel: "todos", records: [args.todo] });
    return { echoed: new Date(args.when.getTime()), status: args.todo.status };
  };
  const handlers: Handlers<Tx> = {
    put: { v1: put, v2: put },
    async change({ args }) {
      return { echoed: args.at };
    },
    async clear() {},
    async ping() {},
    async find() {
      return { todo: null };
    },
    async mark() {},
    async removeMoment() {},
  };
  const loaders: Loaders<Tx> = {
    async todo({ ids, tx }) {
      return ids.map(({ id }) => tx.rows.get(id) ?? null);
    },
    async moment({ ids }) {
      assert.equal(ids[0]?.at.getUTCFullYear(), 2026);
      return ids.map(() => null);
    },
  };
  const native = {
    validateConfig() {},
    async processAction(
      _config: string,
      _owner: string,
      _request: string,
      callback: (request: string) => Promise<string>,
    ) {
      for (const version of [1, 2]) {
        const handled = JSON.parse(
          await callback(
            JSON.stringify({
              op: "handleAction",
              name: "Put",
              version,
              owner: "alice",
              callId: `call-${version}`,
              ordinal: version,
              arguments: {
                todo: {
                  id: "one",
                  title: "saved",
                  at: first,
                  status: "open",
                  note: null,
                },
                when: first,
                statuses: ["open", "closed"],
                note: null,
              },
            }),
          ),
        );
        seen.push(handled);
      }
      const loaded = JSON.parse(
        await callback(
          JSON.stringify({
            op: "load",
            model: "Moment",
            version: 1,
            owner: "alice",
            identities: [{ at: first }],
          }),
        ),
      );
      seen.push(loaded);
      return "{}";
    },
    async processPush() {
      return "{}";
    },
    async processPull() {
      return "{}";
    },
    async settleExternal() {
      return "{}";
    },
    async negotiateLive() {
      return "{}";
    },
    async pullLive() {
      return "{}";
    },
    liveEvent() {
      return "[]";
    },
    liveClose() {},
  };
  const backend = createBackend<Tx>({
    database: {
      transaction: async (body) => body({ rows: new Map() }),
      persistence: () => ({ call: async () => null }),
    },
    authenticate: () => "alice",
    handlers,
    loaders,
    native,
  });
  await backend.action("alice", "{}");
  for (const handled of seen.slice(0, 2) as {
    outputs: { echoed: string; status: string };
    changes: { model: string; identity: Record<string, unknown> }[];
    publications: { records: { identity: Record<string, unknown> }[] }[];
  }[]) {
    assert.equal(handled.outputs.echoed, first);
    assert.equal(handled.outputs.status, "open");
    assert.deepEqual(
      handled.changes.map((ref) => ref.identity),
      [
        { id: "one" },
        { at: first },
        { at: second },
        { at: "2026-01-03T00:00:00.000Z" },
        { at: "2026-01-04T00:00:00.000Z" },
      ],
    );
    assert.deepEqual(handled.publications[0]?.records?.[0]?.identity, {
      id: "one",
    });
  }
  assert.ok(ActionRejected.prototype instanceof Error);
});
