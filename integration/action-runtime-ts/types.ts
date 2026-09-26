import type { Call, CallOutcome, GeneratedClient } from "./client.ts";
import {
  createBackend,
  type MutationContext,
  type MutationHandlerCall,
  type QueryContext,
  type Mutations,
  type Queries,
  type Loaders,
  type PutV1Input,
} from "./backend.ts";
import type { Todo } from "./generated.ts";
import type { Database } from "../../packages/server/index.mts";
import type {
  Call as SdkCall,
  CallOutcome as SdkCallOutcome,
} from "../../packages/client-js/index.mts";

declare const client: GeneratedClient;
declare const todo: Todo;
declare const context: MutationContext<{ rows: Map<string, Todo> }>;
declare const queryContext: QueryContext<{ rows: Map<string, Todo> }>;

const call: Promise<
  Call<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = client.mutations.put({
  todo,
  when: new Date(),
  statuses: ["open"],
  note: null,
});
const direct: Promise<{ todo: Todo | null }> = client.queries.find({
  at: new Date(),
});
const queued: Promise<Call<{ todo: Todo | null }>> =
  client.queries.enqueue.find({ at: new Date() });
const outcome: Promise<
  CallOutcome<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = call.then((handle) => handle.wait());
const sharedCall: Promise<
  SdkCall<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = call;
const sharedOutcome: Promise<
  SdkCallOutcome<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = outcome;
const local: Promise<void> = client.models.todo.create(todo);
const optional = client.mutations.change({ at: new Date() });
const identity = client.mutations.mark({
  moment: { at: new Date(), title: "changed" },
});
const removed: Promise<{ moment: { at: Date } }> =
  client.mutations.call.removeMoment({ moment: { at: new Date() } });
void [
  direct,
  queued,
  outcome,
  sharedCall,
  sharedOutcome,
  local,
  optional,
  identity,
  removed,
];
context.tx.rows.set(todo.id, todo);
context.changes.add(todo);
context.publish({ channel: "todos", records: [todo] });
queryContext.tx.rows.get(todo.id);
void queryContext.callId;
// @ts-expect-error a Query context has no changes
queryContext.changes.add(todo);
// @ts-expect-error a Query context has no publish
queryContext.publish({ channel: "todos" });

type Tx = { rows: Map<string, Todo> };
const queries: Queries<Tx> = {
  find: {
    async v2({ ctx }) {
      ctx.tx.rows.get("one");
      // @ts-expect-error Query handlers cannot report changes
      ctx.changes.add(todo);
      return { todo: { id: "one" } };
    },
  },
};
// @ts-expect-error Query Model outputs are typed identities, not bare keys
const wholeModel: Queries<Tx> = { find: { v2: async () => ({ todo: "one" }) } };
const queryV1: Queries<Tx> = {
  find: {
    v2: async () => ({ todo: null }),
    // @ts-expect-error v1 of Find is a Mutation; the Query map holds only v2
    v1: async () => ({ todo: null }),
  },
};
void [queries, wholeModel, queryV1];
const handlers: Mutations<Tx> = {
  put: {
    async v1({ ctx, args }) {
      ctx.tx.rows.set(args.todo.id, args.todo);
      ctx.changes.add(args.todo);
      ctx.publish({ channel: "todos", records: [args.todo] });
      return {
        echoed: new Date(args.when.getTime()),
        status: args.statuses[0]!,
      };
    },
    async v2({ ctx, args }) {
      ctx.tx.rows.set(args.todo.id, args.todo);
      ctx.changes.add(args.todo);
      ctx.publish({ channel: "todos", records: [args.todo] });
      return {
        echoed: new Date(args.when.getTime()),
        status: args.statuses[0]!,
      };
    },
  },
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
    return ids.map((id) => tx.rows.get(id.id) ?? null);
  },
  async moment({ ids }) {
    return ids.map(() => null);
  },
};
void [handlers, loaders];
declare const database: Database<Tx>;
if (false) {
  const backend = createBackend({
    database,
    authenticate: () => "alice",
    mutations: {
      ...handlers,
      async ping({ ctx }) {
        ctx.tx.rows.set("x", todo);
        // @ts-expect-error inferred application Tx has no missing member
        ctx.tx.missing;
      },
    },
    queries,
    loaders,
  });
  void backend;
  // @ts-expect-error a schema that retains Queries requires the queries map
  createBackend({ database, authenticate: () => "alice", mutations: handlers, loaders });
}
const retained = (call: MutationHandlerCall<Tx, PutV1Input>) =>
  call.args.todo.at.getUTCFullYear();
void retained;

// @ts-expect-error every retained Mutation version must be registered
const missingVersion: Mutations<Tx>["put"] = {
  v2: async () => ({ echoed: new Date(), status: "open" }),
};
void missingVersion;

// @ts-expect-error required nullable ordinary input must be present
client.mutations.put({ todo, when: new Date(), statuses: [] });
// @ts-expect-error DateTime input uses Date
client.queries.find({ at: "2026-01-01T00:00:00Z" });
// @ts-expect-error enum list members are checked
client.mutations.put({ todo, when: new Date(), statuses: ["typo"], note: null });
// @ts-expect-error Find is a Query now; the Mutation namespace no longer has it
client.mutations.find({ at: new Date() });
// @ts-expect-error a direct result has no wait
client.queries.find({ at: new Date() }).then((result) => result.wait());
// @ts-expect-error a durable Call has no result property
client.mutations.ping({}).then((handle) => handle.result);
// @ts-expect-error the old actions namespace is gone
client.actions.ping({});
// @ts-expect-error store keys name explicit Model outputs only
client.queries.find({ at: new Date() }, { store: { missing: false } });
client.queries.find({ at: new Date() }, { store: { todo: false } });
client.queries.enqueue.find({ at: new Date() }, { store: false });
// @ts-expect-error standalone local models have no named mutation method
client.models.todo.mutate({});
client.transaction(async (tx) => {
  // @ts-expect-error transaction models do not watch
  tx.models.todo.watch({}, () => {});
});
client.transaction(async (tx) => {
  // @ts-expect-error transactions cannot submit Mutations
  tx.mutations.ping({});
  // @ts-expect-error transactions cannot run Queries
  tx.queries.find({ at: new Date() });
});
