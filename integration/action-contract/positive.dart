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
const findOutput = FindTodosHandlerOutput(todos: [TodoIdentity(id: 't')], nextCursor: null);

Future<AddTodoHandlerOutput> handle(
  MutationHandlerCall<Object, AddTodoInput> call,
) async {
  final Object context = call.ctx;
  final TodoCreate input = call.args.todo;
  final AddTodoPatchUpdate? patch = call.args.patch;
  context.hashCode;
  input.id;
  patch?.title;
  return handlerOutput;
}

Future<FindTodosHandlerOutput> find(
  QueryHandlerCall<Object, FindTodosInput> call,
) async {
  final String text = call.args.text;
  final String? cursor = call.args.cursor;
  text.length;
  cursor?.length;
  return findOutput;
}

// GetTodos retains v1 as a Mutation and v2 as a Query.
abstract class GetTodosContracts
    implements MutationGetTodosHandlers<Object>, QueryGetTodosHandlers<Object> {}

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
  // Mutations: durable by default, direct under `call`.
  final Call<AddTodoOutput> action = await client.mutations.addTodo(
    todo: created, patch: changed, gone: [deleted], status: null, tags: [],
  );
  final CallStatus status = action.status;
  final CallOutcome<AddTodoOutput> outcome = await action.wait();
  if (outcome is CallSuccess<AddTodoOutput>) {
    final Todo full = outcome.result.todo;
    final Todo? related = outcome.result.relatedTodo;
    final List<Todo> matches = outcome.result.matches;
    full.id;
    related?.id;
    matches.length;
  } else if (outcome is CallFailure<AddTodoOutput>) {
    final String code = outcome.error.code;
    code.length;
  }
  final AddTodoOutput direct = await client.mutations.call.addTodo(
    todo: created, gone: [], status: Status.open, tags: [],
  );
  final RemoveTodoOutput removed = await client.mutations.call.removeTodo(todo: deleted);
  final String removedId = removed.todo.id;
  await client.mutations.call.ping();
  final Call<void> searched = await client.mutations.search(query: null);
  await client.mutations.call.search(query: 'term');
  await client.mutations.call.deleteTodo(todo: deleted);
  await client.mutations.sendEmail(to: 'team@example.test', subject: 'Todo', body: 'Created');
  // Queries: direct by default, durable under `enqueue`.
  final GetTodosOutput todos = await client.queries.getTodos();
  final FindTodosOutput page = await client.queries.findTodos(text: 'design', cursor: null);
  final String? next = page.nextCursor;
  final Call<FindTodosOutput> queued =
      await client.queries.enqueue.findTodos(text: 'design', cursor: next);
  final CallOutcome<FindTodosOutput> queuedOutcome = await queued.wait();
  // store selectors ride beside args on every route; results keep their types.
  final GetTodosOutput unstored =
      await client.queries.getTodos(store: const GetTodosStore.none());
  await client.queries.enqueue.getTodos(store: const GetTodosStore.outputs(todos: false));
  await client.queries.findTodos(text: 'x', cursor: null, store: const FindTodosStore.outputs(todos: false));
  await client.mutations.call.ping(store: const PingStore.none());
  await client.mutations.ping(store: const PingStore.all());
  // A business input named store keeps its name; the selector is outputStore.
  final OpenTodoOutput opened = await client.mutations.call.openTodo(
    store: 'business',
    outputStore: const OpenTodoStore.outputs(suggestions: false),
  );
  final Call<OpenTodoOutput> openCall = await client.mutations.openTodo(
    store: null,
    outputStore: const OpenTodoStore.all(),
  );
  searched.status;
  queuedOutcome.hashCode;
  unstored.todos.length;
  opened.suggestions.length;
  openCall.status;
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
  find;
  useClient;
}
