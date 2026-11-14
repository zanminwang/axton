import type {Call,GeneratedClient} from './client.ts';
import type {AddTodoInput,AddTodoOutput,Todo,TodoIdentity,TodoUpdate,ProjectIdentity} from './generated.ts';
import type {AddTodoV1Input,AddTodoHandlerOutput,StateListV1HandlerOutput,QueryContext,MutationContext,TransactionCall,Options} from './backend.ts';
declare const client:GeneratedClient;declare const call:Call<AddTodoOutput>;declare const input:AddTodoInput;declare const todo:Todo;
// @ts-expect-error Call has no early result
call.result;
// @ts-expect-error status is immutable
call.status='failed';
// @ts-expect-error no direct Mutation lane
client.mutations.call.addTodo(input);
// @ts-expect-error no queued Query lane
client.queries.enqueue.findTodos({text:'x',cursor:null});
// @ts-expect-error no anonymous remote writes
client.mutate;
// @ts-expect-error no Load manager
client.loads;
// @ts-expect-error one Stream selected at open
client.streams;
// @ts-expect-error the Mutation input callback is retired
client.mutations.addTodo(async()=>input);
// @ts-expect-error old local option removed
client.mutations.addTodo(input,{local:async()=>{}});
// @ts-expect-error Mutation has no store option
client.mutations.addTodo(input,{store:false});
// @ts-expect-error request store is boolean
client.queries.findTodos({text:'x',cursor:null},{store:{todos:false}});
// @ts-expect-error nullable input is still required
client.queries.findTodos({text:'x'});
// @ts-expect-error wrong kind namespace
client.mutations.findTodos({text:'x',cursor:null});
// @ts-expect-error no backend reads inside transaction
client.transaction(async tx=>tx.queries.findTodos({text:'x',cursor:null}));
// @ts-expect-error the Mutation input callback is retired inside a transaction too
client.transaction(async tx=>tx.mutations.addTodo(async()=>input));
// @ts-expect-error defaulted nullable memo remains required
client.mutations.addNotes({note:{},many:[]});
// @ts-expect-error explicit nonnullable default cannot be null
client.mutations.addNotes({note:{memo:null,id:null},many:[]});
// @ts-expect-error a Model facade has no create; rows change through Mutations
client.models.note.create({memo:null});
// @ts-expect-error a Model facade has no delete
client.models.todo.delete({id:'t'});
// @ts-expect-error a transaction Model facade has no create
client.transaction(async tx=>tx.models.todo.create(todo));
// @ts-expect-error a transaction Model facade has no update
client.transaction(async tx=>tx.models.todo.update({id:'t'},{title:'x'}));
// @ts-expect-error named update field list excludes the existing state field
const patch:TodoUpdate<'title'>={id:'t',state:'closed'};
// @ts-expect-error composite identity requires every component
const identity:ProjectIdentity={id:'p'};
// @ts-expect-error enum frozen old input excludes archived
const old:AddTodoV1Input={todo:{id:'t',title:'t',state:'archived'},gone:[],status:null,tags:[]};
// @ts-expect-error handler returns identity not full Model
const output:AddTodoHandlerOutput={relatedTodo:{id:"t",title:"full"},matches:[],count:1,state:null};
// @ts-expect-error retained output enum excludes archived
const states:StateListV1HandlerOutput={states:['archived']};
declare const q:QueryContext<object>;declare const m:MutationContext<object>;declare const b:TransactionCall<object>;
// @ts-expect-error Query track only
q.stream.invalidate.todo('t');
// @ts-expect-error Query cannot globally invalidate
q.invalidate.todo('t');
// @ts-expect-error current Stream is a handle
m.stream('User:b');
// @ts-expect-error explicit multiple Streams require a list
m.streams('User:b');
// @ts-expect-error background has no implicit Stream
b.stream.track.todo('t');
// @ts-expect-error complete identity required
m.stream.track.project({id:'p'});
// @ts-expect-error a Model facade has no update
client.models.todo.update({id:'t'},{title:'x'});
void [patch,identity,old,output,states];

// @ts-expect-error EditMany restricts its named update slot to title
client.mutations.editMany({todos:[{id:'t',state:'closed'}]});

// Stream authorization is a required top-level backend option.
declare const withoutAuthorization:Omit<Options<object>,'authorizeStream'>;
// @ts-expect-error a backend without authorizeStream does not compile
const unauthorized:Options<object>=withoutAuthorization;
const authorized:Options<object>={...withoutAuthorization,authorizeStream:()=>true};
void [unauthorized,authorized];
