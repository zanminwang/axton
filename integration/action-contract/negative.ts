import * as actionBackend from "./backend.ts";
import type { ActionCall, GeneratedClient } from "./client.ts";
import type { AddTodoInput, AddTodoOutput, Todo, TodoIdentity, TodoUpdate, ProjectIdentity, PingOutput } from './generated.ts';
import type { AddTodoHandlerOutput, AddTodoV1Input, AddTodoV1HandlerOutput, LinkHandlerOutput, PingHandlerOutput, RemoveTodoHandlerOutput, Handlers, StateListHandlerOutput, StateListV1HandlerOutput } from './backend.ts';

declare const client: GeneratedClient;
declare const concrete: GeneratedClient;
void concrete.actions;
declare const call: ActionCall<AddTodoOutput>;
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
// @ts-expect-error No Action can run inside an application transaction.
client.transaction(async tx => tx.actions.addTodo(input));
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
const missingV2: Pick<Handlers<{}>, 'addTodo'> = { addTodo: { async v1() { return { relatedTodo: null, matches: [], count: 1 }; } } };
void actionBackend.createBackend;
// @ts-expect-error All versioned Action handlers are required.
const missingHandler: Handlers<{}> = { link: { async v1() { return { relatedProject: null }; } }, ping: { async v1() {} } };
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
client.actions.call.openTodo({ store: null }, { store: { count: false } });
// @ts-expect-error store maps reject unknown output names.
client.actions.openTodo({ store: null }, { store: { missing: false } });
// @ts-expect-error store map values are booleans.
client.actions.call.openTodo({ store: null }, { store: { suggestions: 'no' } });
// @ts-expect-error input-bound outputs are not store keys.
client.actions.call.addTodo(input, { store: { todo: false } });
// @ts-expect-error Delete confirmations are not store keys.
client.actions.call.deleteTodo({ todo: { id: 't' } }, { store: { todo: false } });
// @ts-expect-error Actions without eligible outputs accept only a boolean store.
client.actions.call.ping({}, { store: {} });
