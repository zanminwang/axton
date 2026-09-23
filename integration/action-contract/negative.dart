import 'generated.dart';

const todo = Todo(id: 't', title: 'Task', state: Status.open, note: null);
const identity = TodoIdentity(id: 't');
const created = TodoCreate(id: 't', title: 'Task', state: Status.open, note: null);

Future<void> misuse(ActionClientContract client, ActionCall<AddTodoOutput> call) async {
  await client.actions.addTodo(todo: created, gone: [], tags: []); // required nullable status
  await client.actions.addTodo(todo: created, gone: [], status: null, tags: [], status: Status.open); // duplicate shape
  await client.actions.addTodo(todo: created, gone: [], status: null, tags: [], patch: TodoUpdate(id: 't')); // restricted shape
  await client.actions.addTodo(todo: created, gone: [], status: null, tags: [], patch: AddTodoPatchUpdate(id: 't', state: Present(Status.open))); // invalid patch field
  await client.actions.call.addTodo(todo: created, gone: [], status: null, tags: [null]); // non-null list member
  await client.transaction((tx) async {
    tx.actions; // Actions excluded from application transaction
    tx.models.todo.watch(); // watch excluded from transaction
  });
  call.result; // no framework result field
  call.error; // no framework error field
  call.subscribe; // no subscription API
  final AddTodoHandlerOutput scalarIdentity = AddTodoHandlerOutput(relatedTodo: 't', matches: [], count: 1, state: null);
  final AddTodoHandlerOutput fullModel = AddTodoHandlerOutput(relatedTodo: todo, matches: [], count: 1, state: null);
  final AddTodoHandlerOutput missingRelated = AddTodoHandlerOutput(matches: [], count: 1, state: null);
  final AddTodoHandlerOutput invalidList = AddTodoHandlerOutput(relatedTodo: identity, matches: [todo], count: 1, state: null);
  final AddTodoHandlerOutput missingCount = AddTodoHandlerOutput(relatedTodo: null, matches: [], state: null);
  final LinkHandlerOutput wrongComposite = LinkHandlerOutput(relatedProject: TodoIdentity(id: 't'));
  scalarIdentity.hashCode;
  fullModel.hashCode;
  missingRelated.hashCode;
  invalidList.hashCode;
  missingCount.hashCode;
  wrongComposite.hashCode;
}

final oldArchived = AddTodoV1Input(
  todo: AddTodoV1TodoCreate(id: 'old', title: 'Old', state: Status.archived),
  gone: [], status: AddTodoV1Status.open, tags: [],
);

Future<void> missingSearchQuery(ActionClientContract client) async {
  await client.actions.search();
}

final stateListScalar = StateListHandlerOutput(states: Status.open);
final stateListInvalid = StateListHandlerOutput(states: [Status.invalid]);

final oldStateListScalar = StateListV1HandlerOutput(states: StateListV1OutputStatus.open);
final oldStateListArchived = StateListV1HandlerOutput(states: [StateListV1OutputStatus.archived]);
