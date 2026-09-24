import type { ActionCall, ActionOutcome, GeneratedClient } from "./client.ts";
import {
  createBackend,
  type ActionContext,
  type ActionHandlerCall,
  type Handlers,
  type Loaders,
  type PutV1Input,
} from "./backend.ts";
import type { Todo } from "./generated.ts";
import type { Database } from "../../packages/server/index.mts";
import type {
  ActionCall as SdkActionCall,
  ActionOutcome as SdkActionOutcome,
} from "../../packages/client-js/index.mts";

declare const client: GeneratedClient;
declare const todo: Todo;
declare const context: ActionContext<{ rows: Map<string, Todo> }>;

const call: Promise<
  ActionCall<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = client.actions.put({
  todo,
  when: new Date(),
  statuses: ["open"],
  note: null,
});
const direct: Promise<{ todo: Todo | null }> = client.actions.call.find({
  at: new Date(),
});
const outcome: Promise<
  ActionOutcome<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = call.then((handle) => handle.wait());
const sharedCall: Promise<
  SdkActionCall<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = call;
const sharedOutcome: Promise<
  SdkActionOutcome<{ todo: Todo; echoed: Date; status: "open" | "closed" }>
> = outcome;
const local: Promise<void> = client.models.todo.create(todo);
const optional = client.actions.change({ at: new Date() });
const identity = client.actions.mark({
  moment: { at: new Date(), title: "changed" },
});
const removed: Promise<{ moment: { at: Date } }> =
  client.actions.call.removeMoment({ moment: { at: new Date() } });
void [
  direct,
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

type Tx = { rows: Map<string, Todo> };
const handlers: Handlers<Tx> = {
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
    handlers: {
      ...handlers,
      async ping({ ctx }) {
        ctx.tx.rows.set("x", todo);
        // @ts-expect-error inferred application Tx has no missing member
        ctx.tx.missing;
      },
    },
    loaders,
  });
  void backend;
}
const retained = (call: ActionHandlerCall<Tx, PutV1Input>) =>
  call.args.todo.at.getUTCFullYear();
void retained;

// @ts-expect-error every retained Action version must be registered
const missingVersion: Handlers<Tx>["put"] = {
  v2: async () => ({ echoed: new Date(), status: "open" }),
};
void missingVersion;

// @ts-expect-error required nullable ordinary input must be present
client.actions.put({ todo, when: new Date(), statuses: [] });
// @ts-expect-error DateTime input uses Date
client.actions.call.find({ at: "2026-01-01T00:00:00Z" });
// @ts-expect-error enum list members are checked
client.actions.put({ todo, when: new Date(), statuses: ["typo"], note: null });
// @ts-expect-error standalone local models have no named mutation method
client.models.todo.mutate({});
client.transaction(async (tx) => {
  // @ts-expect-error transaction models do not watch
  tx.models.todo.watch({}, () => {});
});
client.transaction(async (tx) => {
  // @ts-expect-error transactions cannot submit durable Actions
  tx.actions.ping({});
});
