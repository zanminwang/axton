import type { ActionCall, ActionClientContract, ActionOutcome, AddTodoInput, AddTodoOutput, TodoCreate, TodoUpdate, TodoDelete, TodoIdentity, ProjectIdentity, PingOutput } from './generated.ts';
import type { AddTodoHandlerOutput, AddTodoV1Input, AddTodoV1HandlerOutput, PingHandlerOutput, RemoveTodoHandlerOutput, Handlers, Loaders, StateListHandlerOutput, StateListV1HandlerOutput } from './backend.ts';

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

async function clientContract(client: ActionClientContract) {
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
  const call: ActionCall<AddTodoOutput> = await client.actions.addTodo(input);
  const status: 'pending' | 'succeeded' | 'failed' = call.status;
  const outcome: ActionOutcome<AddTodoOutput> = await call.wait();
  if (outcome.error === null) {
    const todo: string = outcome.result.todo.title;
    void todo;
  } else {
    const code: string = outcome.error.code;
    void code;
  }
  const final: AddTodoOutput = await client.actions.call.addTodo(input);
  const removed = await client.actions.call.removeTodo({ todo: { id: 't' } });
  const removedId: string = removed.todo.id;
  const noOutput: PingOutput = await client.actions.call.ping({});
  await client.actions.call.deleteTodo({ todo: deletion });
  await client.actions.sendEmail({ to: 'team@example.test', subject: 'Todo', body: 'Created' });
  const selected = await client.actions.call.getTodos({});
  const rows: readonly { id: string }[] = selected.todos;
  void [status, final, removedId, noOutput, rows];
}

type Ctx = { db: unknown };
const pingHandlerResult: PingHandlerOutput = undefined;
const removeHandlerResult: RemoveTodoHandlerOutput = undefined;
const handlers: Handlers<Ctx> = {
  addTodo: { async v1({ ctx, args }) { void ctx.db; void args.todo.id; return { relatedTodo: { id: args.todo.id }, matches: [], count: 1 }; }, async v2({ ctx, args }) { void ctx.db; void args.todo.note; return handlerOutput; } },
  link: { async v1({ args }) { return { relatedProject: { tenantId: args.project.tenantId, id: args.project.id } }; } },
  ping: { async v1({ ctx, args }) { void ctx.db; void args; } },
  removeTodo: { async v1({ args }) { void args.todo.id; } },
  search: { async v1({ args }) { void args.query; } },
  deleteTodo: { async v1({ args }) { void args.todo.id; } },
  sendEmail: { async v1({ args }) { void args.to; void args.subject; void args.body; } },
  getTodos: { async v1() { return { todos: [{ id: 't' }] }; } },
  stateList: { async v1() { return oldStateListOutput; }, async v2() { return stateListOutput; } },
};
const loaders: Loaders<Ctx> = {
  todo: { async v1() { return []; }, async v2() { return []; } },
  project: async () => [],
};
void [handlers, loaders, clientContract, composite, oldInput, oldOutput, pingHandlerResult, removeHandlerResult, oldStateListOutput];
