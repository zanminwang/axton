import assert from "node:assert/strict";
import test from "node:test";
import {
  CallRejected,
  Moment,
  createBackend,
  type MutationHandlerCall,
  type Mutations,
  type Queries,
  type Loaders,
  type PutV1Input,
} from "./backend.ts";
import type { Todo } from "./generated.ts";

test("generated backend decodes Date values and keeps canonical Model references", async () => {
  const first = "2026-01-01T00:00:00.000Z";
  const second = "2026-01-02T00:00:00.000Z";
  const seen: unknown[] = [];
  type Tx = { rows: Map<string, Todo> };
  const put = async ({ ctx, args }: MutationHandlerCall<Tx, PutV1Input>) => {
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
  const mutations: Mutations<Tx> = {
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
  const queries: Queries<Tx> = {
    find: {
      async v2() {
        return { todo: null };
      },
    },
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
    mutations,
    queries,
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
  assert.ok(CallRejected.prototype instanceof Error);
});

type Tx = { rows: Map<string, Todo> };
/** A native stub that sends each request to the host and keeps its answer. */
function nativeHost(requests: object[], answers: unknown[]) {
  return {
    validateConfig() {},
    async processAction(
      _config: string,
      _owner: string,
      _request: string,
      callback: (request: string) => Promise<string>,
    ) {
      for (const request of requests)
        answers.push(JSON.parse(await callback(JSON.stringify(request))));
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
}
const database = {
  transaction: async <R,>(body: (tx: Tx) => Promise<R>) =>
    body({ rows: new Map() }),
  persistence: () => ({ call: async () => null }),
};
const loaders: Loaders<Tx> = {
  async todo({ ids }) {
    return ids.map(() => null);
  },
  async moment({ ids }) {
    return ids.map(() => null);
  },
};
function mutationHandlers(): Mutations<Tx> {
  const none = async () => {};
  return {
    put: {
      v1: async ({ args }) => ({ echoed: args.when, status: "open" }),
      v2: async ({ args }) => ({ echoed: args.when, status: "open" }),
    },
    change: async ({ args }) => ({ echoed: args.at }),
    clear: none,
    ping: none,
    find: async () => ({ todo: null }),
    mark: none,
    removeMoment: none,
  };
}

test("Query handlers receive no effect capabilities and settle without effects", async () => {
  const seen: Record<string, unknown>[] = [];
  const answers: unknown[] = [];
  const at = "2026-01-01T00:00:00.000Z";
  const call = (name: string, version: number) => ({
    op: "handleAction",
    name,
    version,
    owner: "alice",
    callId: `${name}-${version}`,
    ordinal: 1,
    arguments: { at },
  });
  const backend = createBackend<Tx>({
    database,
    authenticate: () => "alice",
    mutations: {
      ...mutationHandlers(),
      find: async ({ ctx, args }) => {
        seen.push({ kind: "mutation", keys: Object.keys(ctx).sort() });
        ctx.changes.add({ model: "Todo", identity: { id: "one" } });
        assert.ok(args.at instanceof Date);
        return { todo: { id: "one" } };
      },
    },
    queries: {
      find: {
        async v2({ ctx, args }) {
          seen.push({ kind: "query", keys: Object.keys(ctx).sort() });
          assert.equal(ctx.userId, "alice");
          assert.equal(ctx.callId, "Find-2");
          assert.ok(args.at instanceof Date);
          // @ts-expect-error a Query context has no changes
          assert.equal(ctx.changes, undefined);
          // @ts-expect-error a Query context has no publish
          assert.equal(ctx.publish, undefined);
          return { todo: { id: "one" } };
        },
      },
    },
    loaders,
    native: nativeHost([call("Find", 1), call("Find", 2)], answers),
  });
  await backend.action("alice", "{}");
  assert.deepEqual(seen, [
    { kind: "mutation", keys: ["callId", "changes", "publish", "tx", "userId"] },
    { kind: "query", keys: ["callId", "tx", "userId"] },
  ]);
  assert.deepEqual(answers, [
    {
      outputs: { todo: { id: "one" } },
      changes: [{ model: "Todo", identity: { id: "one" } }],
      publications: [],
    },
    { outputs: { todo: { id: "one" } }, changes: [], publications: [] },
  ]);
});

test("registration is checked per kind at startup: missing, extra and wrong-kind", () => {
  const start = (
    options: Partial<Parameters<typeof createBackend<Tx>>[0]>,
  ): unknown =>
    createBackend<Tx>({
      database,
      authenticate: () => "alice",
      mutations: mutationHandlers(),
      queries: { find: { v2: async () => ({ todo: null }) } },
      loaders,
      native: nativeHost([], []),
      ...options,
    } as Parameters<typeof createBackend<Tx>>[0]);
  start({});
  const cases: [Partial<Parameters<typeof createBackend<Tx>>[0]>, RegExp][] = [
    [{ queries: undefined }, /Missing query find for Find v2/],
    [{ queries: {} as Queries<Tx> }, /Missing query find for Find v2/],
    // A bare function is v1 shorthand; Find's only Query version is v2.
    [
      { queries: { find: async () => ({ todo: null }) } as unknown as Queries<Tx> },
      /Query find must register v2 of Find; a function registers v1 only/,
    ],
    [
      {
        queries: {
          find: { v2: async () => ({ todo: null }) },
          ping: async () => {},
        } as unknown as Queries<Tx>,
      },
      /queries\.ping: Ping v1 \(mutation\) retains no query version; register it under mutations/,
    ],
    [
      {
        mutations: {
          ...mutationHandlers(),
          search: async () => {},
        } as unknown as Mutations<Tx>,
      },
      /Unknown mutation search: no retained mutation search/,
    ],
    [
      {
        mutations: {
          ...mutationHandlers(),
          find: { v1: async () => ({ todo: null }), v2: async () => ({ todo: null }) },
        } as unknown as Mutations<Tx>,
      },
      /Unknown mutation find\.v2 for Find: retained mutation versions are v1/,
    ],
    [
      { handlers: { find: async () => ({ todo: null }) } } as never,
      /Handler find names Find v1 \(mutation\), v2 \(query\)/,
    ],
    [
      {
        queries: {
          find: { v1: async () => ({ todo: null }), v2: async () => ({ todo: null }) },
        } as unknown as Queries<Tx>,
      },
      /Unknown query find\.v1 for Find: retained query versions are v2/,
    ],
    [
      { handlers: { ping: async () => {} } } as never,
      /Handler ping names Ping v1 \(mutation\); register each version under mutations or queries by its kind/,
    ],
  ];
  for (const [options, message] of cases)
    assert.throws(() => start(options), message);
});
