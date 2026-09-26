import * as actionBackend from "./backend.ts";
import type { Call, GeneratedClient } from "./client.ts";
import type { AddTodoInput, AddTodoOutput, Note, NoteCreate, Todo, TodoIdentity, TodoUpdate, ProjectIdentity, PingOutput } from './generated.ts';
import type { AddNotesInput as AddNotesHandlerInput, AddTodoHandlerOutput, AddTodoV1Input, AddTodoV1HandlerOutput, FindTodosHandlerOutput, LinkHandlerOutput, PingHandlerOutput, QueryContext, RemoveTodoHandlerOutput, Mutations, Queries, StateListHandlerOutput, StateListV1HandlerOutput } from './backend.ts';

declare const client: GeneratedClient;
declare const concrete: GeneratedClient;
void [concrete.mutations, concrete.queries];
declare const call: Call<AddTodoOutput>;
declare const todo: Todo;
declare const input: AddTodoInput;
// @ts-expect-error A live call has no result property; wait for its outcome.
call.result;
// @ts-expect-error A live call has no error property; wait for its outcome.
call.error;
// @ts-expect-error Call status is read-only.
call.status = 'failed';
// @ts-expect-error No output shortcut exists on a call handle.
call.output;
// @ts-expect-error No cancel method exists on a call handle.
call.cancel();
// @ts-expect-error No Mutation can run inside an application transaction.
client.transaction(async tx => tx.mutations.addTodo(input));
// @ts-expect-error No Query can run inside an application transaction.
client.transaction(async tx => tx.queries.findTodos({ text: 'x', cursor: null }));
// @ts-expect-error The retired actions namespace does not exist.
void concrete.actions;
// @ts-expect-error A Query is not under mutations.
client.mutations.findTodos({ text: 'x', cursor: null });
// @ts-expect-error A Mutation is not under queries.
client.queries.addTodo(input);
// @ts-expect-error A direct Mutation is not under queries.enqueue.
client.queries.enqueue.addTodo(input);
// @ts-expect-error A Query has no direct `call` member; it is direct by default.
client.queries.call.findTodos({ text: 'x', cursor: null });
// @ts-expect-error A Mutation has no `enqueue` member; it is durable by default.
client.mutations.enqueue.addTodo(input);
// @ts-expect-error A direct result has no wait; it is already final.
client.queries.findTodos({ text: 'x', cursor: null }).then(result => result.wait());
// @ts-expect-error A direct Mutation result is its output, not a Call.
const directCall: Promise<Call<AddTodoOutput>> = client.mutations.call.addTodo(input);
// @ts-expect-error A durable Query resolves to a Call, not its output.
const queuedOutput: Promise<{ todos: Todo[] }> = client.queries.enqueue.findTodos({ text: 'x', cursor: null });
// @ts-expect-error A nullable Query input is still a required argument.
client.queries.findTodos({ text: 'x' });
void [directCall, queuedOutput];
declare const queryContext: QueryContext<{}>;
// @ts-expect-error A Query context has no changes.
queryContext.changes;
// @ts-expect-error A Query context has no publish.
queryContext.publish({ channel: 'todos' });
// @ts-expect-error A Query handler cannot use Mutation effects.
const effectfulQuery: Queries<{}>['findTodos'] = async ({ ctx }) => { ctx.changes.add({ model: 'Todo', identity: { id: 'x' } }); return { todos: [], nextCursor: null }; };
// @ts-expect-error Query Model outputs are identity objects.
const bareQueryOutput: FindTodosHandlerOutput = { todos: ['x'], nextCursor: null };
const wrongKindVersion: Queries<{}>['getTodos'] = {
  async v2() { return { todos: [] }; },
  // @ts-expect-error The Query version of GetTodos is v2; v1 is a Mutation.
  async v1() { return { todos: [] }; },
};
// @ts-expect-error A Mutation map does not register a Query-only name.
const queryInMutations: Pick<Mutations<{}>, 'findTodos'> = {};
void [effectfulQuery, bareQueryOutput, wrongKindVersion, queryInMutations];
// @ts-expect-error Transaction models cannot watch.
client.transaction(async tx => tx.models.todo.watch({}, () => {}));
// @ts-expect-error Ordinary nullable args are required-present.
const missingNullable: AddTodoInput = { todo, gone: [], tags: [] };
// @ts-expect-error Patch fields outside the restricted selection are rejected.
const badPatch: TodoUpdate<'title'> = { id: 'x', state: 'open' };
// @ts-expect-error Wrong identity fields for composite Project.
const badProject: ProjectIdentity = { id: 'p' };
// @ts-expect-error Model output requires an identity object, not a bare key.
const bare: AddTodoHandlerOutput = { relatedTodo: 'id', matches: [], count: 1, state: null };
// @ts-expect-error Excess fields in an inline identity object are rejected.
const full: AddTodoHandlerOutput = { relatedTodo: { id: 'x', title: 'extra' }, matches: [], count: 1, state: null };
// @ts-expect-error A Project identity cannot replace a Todo identity.
const wrongModel: AddTodoHandlerOutput = { relatedTodo: { tenantId: 't', id: 'x' }, matches: [], count: 1, state: null };
// @ts-expect-error Explicit handler outputs must include required count.
const missingCount: AddTodoHandlerOutput = { relatedTodo: null, matches: [], state: null };
// @ts-expect-error Model output lists require identity objects in order.
const badList: AddTodoHandlerOutput = { relatedTodo: null, matches: ['id'], count: 1, state: null };
// @ts-expect-error Composite identities need both key fields.
const badComposite: LinkHandlerOutput = { relatedProject: { id: 'x' } };
// @ts-expect-error Extra inline Model fields are not identity fields.
const fullComposite: LinkHandlerOutput = { relatedProject: { tenantId: 't', id: 'x', title: 'extra' } };
// @ts-expect-error Handler scalar outputs keep their declared type.
const wrongScalar: AddTodoHandlerOutput = { relatedTodo: null, matches: [], count: 'one', state: null };
// @ts-expect-error Retained v1 does not accept the new enum case.
const oldEnum: AddTodoV1Input = { todo: { id: 'x', title: 'Old', state: 'archived' }, gone: [], status: 'open', tags: [] };
// @ts-expect-error Retained v1 create input excludes v2 Model fields.
const oldModel: AddTodoV1Input = { todo: { id: 'x', title: 'Old', state: 'open', note: null }, gone: [], status: 'open', tags: [] };
// @ts-expect-error Retained v1 output has its original field set.
const oldOutput: AddTodoV1HandlerOutput = { relatedTodo: null, matches: [], count: 1, state: 'open' };
// @ts-expect-error A no-output client result is void.
const badPing: PingOutput = { unexpected: true };
// @ts-expect-error A no-output handler must return void.
const badPingHandler: PingHandlerOutput = { unexpected: true };
// @ts-expect-error An implicit-only Action handler must return void.
const badRemoveHandler: RemoveTodoHandlerOutput = { todo: { id: 'x' } };
// @ts-expect-error A retained v2 handler cannot be omitted.
const missingV2: Pick<Mutations<{}>, 'addTodo'> = { addTodo: { async v1() { return { relatedTodo: null, matches: [], count: 1 }; } } };
void actionBackend.createBackend;
// @ts-expect-error All versioned Mutation handlers are required.
const missingHandler: Mutations<{}> = { link: { async v1() { return { relatedProject: null }; } }, ping: { async v1() {} } };
void [missingNullable, badPatch, badProject, bare, full, wrongModel, missingCount, badList, badComposite, fullComposite, wrongScalar, oldEnum, oldModel, oldOutput, badPing, badPingHandler, badRemoveHandler, missingV2, missingHandler];

// @ts-expect-error enum-list output rejects a scalar
const stateListScalar: StateListHandlerOutput = { states: 'open' };
// @ts-expect-error enum-list output rejects an invalid member
const stateListInvalid: StateListHandlerOutput = { states: ['invalid'] };
void [stateListScalar, stateListInvalid];

// @ts-expect-error retained v1 enum-list excludes the new case
const oldStateListArchived: StateListV1HandlerOutput = { states: ['archived'] };
// @ts-expect-error retained v1 enum-list rejects a scalar
const oldStateListScalar: StateListV1HandlerOutput = { states: 'open' };
void [oldStateListArchived, oldStateListScalar];

// @ts-expect-error store maps name explicit Model outputs, not scalar outputs.
client.mutations.call.openTodo({ store: null }, { store: { count: false } });
// @ts-expect-error store maps reject unknown output names.
client.mutations.openTodo({ store: null }, { store: { missing: false } });
// @ts-expect-error store map values are booleans.
client.mutations.call.openTodo({ store: null }, { store: { suggestions: 'no' } });
// @ts-expect-error input-bound outputs are not store keys.
client.mutations.call.addTodo(input, { store: { todo: false } });
// @ts-expect-error Delete confirmations are not store keys.
client.mutations.call.deleteTodo({ todo: { id: 't' } }, { store: { todo: false } });
// @ts-expect-error Operations without eligible outputs accept only a boolean store.
client.mutations.call.ping({}, { store: {} });
// @ts-expect-error Query store maps reject scalar outputs.
client.queries.findTodos({ text: 'x', cursor: null }, { store: { nextCursor: false } });
// @ts-expect-error Queued Query store maps reject unknown outputs.
client.queries.enqueue.findTodos({ text: 'x', cursor: null }, { store: { missing: true } });
// Creation defaults (#27): only client create inputs admit omission.
// @ts-expect-error A field without a creation default is still required.
const missingMemo: NoteCreate = {};
// @ts-expect-error An omitted field is left out, not set to undefined.
const undefinedTag: NoteCreate = { memo: null, tag: undefined };
// @ts-expect-error The full Model type stays complete.
const partialNote: Note = { memo: null };
// @ts-expect-error A handler's create argument is the complete, expanded record.
const partialHandlerArgs: AddNotesHandlerInput = { note: { memo: null }, many: [] };
// @ts-expect-error Local create takes the same create input.
client.models.note.create({ body: 'x' });
void [missingMemo, undefinedTag, partialNote, partialHandlerArgs];
