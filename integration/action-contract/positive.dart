import 'generated.dart';

const todo = Todo(id: 't', title: 'Task', state: Status.open, note: null);
const identity = TodoIdentity(id: 't');
const created = TodoCreate(id: 't', title: 'Task', state: Status.open, note: null);
const changed = AddTodoPatchUpdate(id: 't', title: Present('Renamed'));
const clearNote = TodoPatch(note: Present<String?>(null));
const deleted = TodoDelete(id: 't');
const composite = ProjectIdentity(tenantId: 'tenant', id: 'project');
const oldInput = AddTodoV1Input(
  todo: AddTodoV1TodoCreate(id: 'old', title: 'Old', state: AddTodoV1Status.closed),
  gone: [], status: AddTodoV1Status.open, tags: [],
);
const oldOutput = AddTodoV1HandlerOutput(
  relatedTodo: TodoV1Identity(id: 'old'), matches: [], count: 1,
);
const handlerOutput = AddTodoHandlerOutput(
  relatedTodo: TodoIdentity(id: 't'), matches: [TodoIdentity(id: 't')],
  count: 1, state: null,
);
const linkOutput = LinkHandlerOutput(
  relatedProject: ProjectIdentity(tenantId: 'tenant', id: 'project'),
);
const stateListOutput = StateListHandlerOutput(states: [Status.open, Status.closed, Status.archived]);
const oldStateListOutput = StateListV1HandlerOutput(states: [StateListV1OutputStatus.open, StateListV1OutputStatus.closed]);
const getTodosOutput = GetTodosHandlerOutput(todos: [TodoIdentity(id: 't')]);

Future<AddTodoHandlerOutput> handle(
  ActionHandlerCall<Object, AddTodoInput> call,
) async {
  final Object context = call.ctx;
  final TodoCreate input = call.args.todo;
  final AddTodoPatchUpdate? patch = call.args.patch;
  context.hashCode;
  input.id;
  patch?.title;
  return handlerOutput;
}

Future<void> useClient(GeneratedClient client) async {
  await client.models.todo.create(todo);
  await client.models.todo.update(identity, const TodoPatch(title: Present('New')));
  await client.models.todo.update(identity, clearNote);
  await client.models.todo.delete(identity);
  final Todo? found = await client.models.todo.get(identity);
  final List<Todo> rows = await client.models.todo.query();
  final Stream<List<Todo>> stream = client.models.todo.watch();
  await client.transaction((tx) async {
    await tx.models.todo.create(todo);
    await tx.models.todo.update(identity, const TodoPatch(title: Present('Local')));
    await tx.models.todo.delete(identity);
    await tx.models.todo.get(identity);
    await tx.models.todo.query();
  });
  final ActionCall<AddTodoOutput> action = await client.actions.addTodo(
    todo: created, patch: changed, gone: [deleted], status: null, tags: [],
  );
  final ActionStatus status = action.status;
  final ActionOutcome<AddTodoOutput> outcome = await action.wait();
  if (outcome is ActionSuccess<AddTodoOutput>) {
    final Todo full = outcome.result.todo;
    final Todo? related = outcome.result.relatedTodo;
    final List<Todo> matches = outcome.result.matches;
    full.id;
    related?.id;
    matches.length;
  } else if (outcome is ActionFailure<AddTodoOutput>) {
    final String code = outcome.error.code;
    code.length;
  }
  final AddTodoOutput direct = await client.actions.call.addTodo(
    todo: created, gone: [], status: Status.open, tags: [],
  );
  final RemoveTodoOutput removed = await client.actions.call.removeTodo(todo: deleted);
  final String removedId = removed.todo.id;
  await client.actions.call.ping();
  await client.actions.search(query: null);
  await client.actions.call.search(query: 'term');
  await client.actions.call.deleteTodo(todo: deleted);
  await client.actions.sendEmail(to: 'team@example.test', subject: 'Todo', body: 'Created');
  final GetTodosOutput todos = await client.actions.call.getTodos();
  final List<Todo> selected = todos.todos;
  found?.id;
  rows.length;
  stream.hashCode;
  status.name;
  direct.count;
  removedId.length;
  selected.length;
}

void main() {
  composite.id;
  oldInput.todo.id;
  oldOutput.count;
  linkOutput.relatedProject?.id;
  getTodosOutput.todos.length;
  stateListOutput.states.length;
  oldStateListOutput.states.length;
  handle;
  useClient;
}
