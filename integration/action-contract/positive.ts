import type { Call, CallOutcome, GeneratedClient } from './client.ts';
import type { OpenTodoOutput, AddTodoInput, AddTodoOutput, FindTodosOutput, TodoCreate, TodoUpdate, TodoDelete, TodoIdentity, ProjectIdentity, PingOutput } from './generated.ts';
import type { AddTodoHandlerOutput, AddTodoV1Input, AddTodoV1HandlerOutput, FindTodosHandlerOutput, GetTodosV1HandlerOutput, MutationContext, PingHandlerOutput, QueryContext, RemoveTodoHandlerOutput, Mutations, Queries, Loaders, StateListHandlerOutput, StateListV1HandlerOutput } from './backend.ts';

const created: TodoCreate = { id: 't', title: 'Task', state: 'open', note: null };
const patch: TodoUpdate<'title'> = { id: 't', title: 'Renamed' };
const deletion: TodoDelete = { id: 't' };
const input: AddTodoInput = { todo: created, patch, gone: [deletion], status: null, tags: [] };
const identity: TodoIdentity = { id: 't' };
const composite: ProjectIdentity = { tenantId: 'tenant', id: 'project' };
const oldInput: AddTodoV1Input = { todo: { id: 'old', title: 'Old', state: 'closed' }, gone: [], status: 'open', tags: [] };
const oldOutput: AddTodoV1HandlerOutput = { relatedTodo: { id: 'old' }, matches: [], count: 1 };
const stateListOutput: StateListHandlerOutput = { states: ['open', 'closed', 'archived'] };
const oldStateListOutput: StateListV1HandlerOutput = { states: ['open', 'closed'] };
const handlerOutput: AddTodoHandlerOutput = { relatedTodo: identity, matches: [identity], count: 1, state: null };

async function clientContract(client: GeneratedClient) {
  await client.models.todo.create(created);
  await client.models.todo.update(identity, { title: 'New' });
  await client.models.todo.delete(identity);
  await client.models.todo.get(identity);
  await client.models.todo.query();
  client.models.todo.watch({}, () => {});
  await client.transaction(async tx => {
    await tx.models.todo.create(created);
    await tx.models.todo.update(identity, { title: 'New' });
    await tx.models.todo.delete(identity);
    await tx.models.todo.get(identity);
    await tx.models.todo.query();
  });
  // Mutations: durable by default, direct under `call`.
  const call: Call<AddTodoOutput> = await client.mutations.addTodo(input);
  const status: 'pending' | 'succeeded' | 'failed' = call.status;
  const outcome: CallOutcome<AddTodoOutput> = await call.wait();
  if (outcome.error === null) {
    const todo: string = outcome.result.todo.title;
    void todo;
  } else {
    const code: string = outcome.error.code;
    void code;
  }
  const final: AddTodoOutput = await client.mutations.call.addTodo(input);
  const removed = await client.mutations.call.removeTodo({ todo: { id: 't' } });
  const removedId: string = removed.todo.id;
  const noOutput: PingOutput = await client.mutations.call.ping({});
  await client.mutations.call.deleteTodo({ todo: deletion });
  const email: Call<void> = await client.mutations.sendEmail({ to: 'team@example.test', subject: 'Todo', body: 'Created' });
  // Queries: direct by default, durable under `enqueue`.
  const selected = await client.queries.getTodos({});
  const rows: readonly { id: string }[] = selected.todos;
  const found: FindTodosOutput = await client.queries.findTodos({ text: 'design', cursor: null });
  const next: string | null = found.nextCursor;
  const queued: Call<FindTodosOutput> = await client.queries.enqueue.findTodos({ text: 'design', cursor: next });
  const queuedOutcome: CallOutcome<FindTodosOutput> = await queued.wait();
  // store is an invocation option beside args on every route; results keep their types.
  const opened: OpenTodoOutput = await client.mutations.call.openTodo({ store: 'business' }, { store: false });
  const suggestion: string | undefined = opened.suggestions[0]?.title;
  await client.mutations.call.openTodo({ store: null }, { store: { suggestions: false } });
  await client.mutations.call.openTodo({ store: null }, { store: { mainTodo: true, related: false } });
  const openCall: Call<OpenTodoOutput> = await client.mutations.openTodo({ store: null }, { store: true });
  const openOutcome: CallOutcome<OpenTodoOutput> = await openCall.wait();
  await client.mutations.addTodo(input, { store: { matches: false } });
  await client.queries.getTodos({}, {});
  await client.queries.findTodos({ text: 'x', cursor: null }, { store: false });
  await client.queries.enqueue.findTodos({ text: 'x', cursor: null }, { store: { todos: false } });
  await client.queries.enqueue.getTodos({}, { store: true });
  await client.mutations.call.ping({}, { store: false });
  await client.mutations.deleteTodo({ todo: deletion }, { store: true });
  // once reuses a saved complete result; refresh replaces it; invalidate discards it.
  const cachedTodos: FindTodosOutput = await client.queries.findTodos({ text: 'x', cursor: null }, { once: true });
  await client.queries.findTodos({ text: 'x', cursor: null }, { once: true, refresh: true, store: false });
  await client.queries.findTodos({ text: 'x', cursor: null }, { once: false });
  await client.queries.getTodos({}, { once: true, store: { todos: false } });
  const invalidated: void = await client.queries.invalidate.findTodos({ text: 'x', cursor: null });
  await client.queries.invalidate.getTodos({});
  void [cachedTodos, invalidated];
  void [status, final, removedId, noOutput, email, rows, suggestion, openOutcome, queuedOutcome];
}

type Tx = { db: unknown };
const pingHandlerResult: PingHandlerOutput = undefined;
const removeHandlerResult: RemoveTodoHandlerOutput = undefined;
const mutationContext = (ctx: MutationContext<Tx>) => [ctx.tx, ctx.userId, ctx.callId, ctx.changes, ctx.publish];
const queryContext = (ctx: QueryContext<Tx>) => [ctx.tx, ctx.userId, ctx.callId];
const findOutput: FindTodosHandlerOutput = { todos: [{ id: 't' }], nextCursor: null };
const oldGetTodos: GetTodosV1HandlerOutput = { todos: [] };
const handlers: Mutations<Tx> = {
  addTodo: { async v1({ ctx, args }) { void ctx.tx.db; void args.todo.id; return { relatedTodo: { id: args.todo.id }, matches: [], count: 1 }; }, async v2({ ctx, args }) { void ctx.tx.db; void args.todo.note; return handlerOutput; } },
  link: { async v1({ args }) { return { relatedProject: { tenantId: args.project.tenantId, id: args.project.id } }; } },
  ping: { async v1({ ctx, args }) { void ctx.tx.db; void args; } },
  removeTodo: { async v1({ args }) { void args.todo.id; } },
  openTodo: { async v1({ args }) { void args.store; return { mainTodo: { id: 't' }, suggestions: [], related: null, count: 0 }; } },
  search: { async v1({ args }) { void args.query; } },
  deleteTodo: { async v1({ args }) { void args.todo.id; } },
  sendEmail: { async v1({ args }) { void args.to; void args.subject; void args.body; } },
  // v1 of GetTodos stays a Mutation; its v2 is registered as a Query.
  getTodos: async ({ ctx }) => { ctx.changes.add({ model: 'Todo', identity: { id: 't' } }); return oldGetTodos; },
  stateList: { async v1() { return oldStateListOutput; }, async v2() { return stateListOutput; } },
};
const queries: Queries<Tx> = {
  findTodos: async ({ ctx, args }) => { void ctx.tx.db; void ctx.userId; void args.text; void args.cursor; return findOutput; },
  getTodos: { async v2({ ctx }) { void ctx.callId; return { todos: [{ id: 't' }] }; } },
};
const loaders: Loaders<Tx> = {
  todo: { async v1() { return []; }, async v2() { return []; } },
  project: async () => [],
};
void [handlers, queries, mutationContext, queryContext, loaders, clientContract, composite, oldInput, oldOutput, pingHandlerResult, removeHandlerResult, oldStateListOutput];
