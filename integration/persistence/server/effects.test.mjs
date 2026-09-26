import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createEffects} from '../../../packages/server/effects.mts';
import {Todo,Moment,Pin} from '../../action-runtime-ts/backend.ts';

// The real compiled schema: Todo {id String}, Moment {at DateTime} and the
// composite Pin {todo String, at DateTime}.
const {schema}=JSON.parse(await readFile(new URL('../../action-runtime-ts/backend.json',import.meta.url),'utf8'));
const models=schema.models;
const fresh=()=>createEffects(models);
const empty={changes:[],memberships:[]};
const add=(channel,model,identity)=>({channel,model,identity,present:true});
const remove=(channel,model,identity)=>({channel,model,identity,present:false});

test('declarations snapshot identities at the call, in declaration order',()=>{
 const effects=fresh();
 const identity={id:'A'};
 const channel=effects.channel('project:1');
 channel.todo.add(identity);
 identity.id='B';
 effects.touch.todo({id:'A'});
 assert.deepEqual(effects.settlement(),{
  changes:[{model:'Todo',identity:{id:'A'}}],
  memberships:[{channel:'project:1',model:'Todo',identity:{id:'A'},present:true}],
 });
});

test('membership declarations keep their order and repeats; the engine reduces them',()=>{
 const effects=fresh();
 const a=effects.channel('a'),b=effects.channel('b');
 a.todo.add({id:'1'});
 b.todo.remove({id:'2'});
 a.todo.add({id:'1'});
 a.todo.remove({id:'1'});
 effects.channel('a').todo.add({id:'1'});
 assert.deepEqual(effects.settlement().memberships,[
  add('a','Todo',{id:'1'}),remove('b','Todo',{id:'2'}),add('a','Todo',{id:'1'}),
  remove('a','Todo',{id:'1'}),add('a','Todo',{id:'1'}),
 ]);
 assert.deepEqual(effects.settlement().changes,[],'membership alone declares no change');
});

test('a touch keeps one change per record, after the private seeds',()=>{
 const effects=fresh();
 effects.seed({model:'Todo',identity:{id:'seeded'}});
 effects.touch.todo({id:'x'});
 effects.touch.todo({id:'seeded'});
 effects.touch.todo({id:'x',title:'ignored'});
 assert.deepEqual(effects.settlement(),{changes:[
  {model:'Todo',identity:{id:'seeded'}},{model:'Todo',identity:{id:'x'}},
 ],memberships:[]});
 assert.equal('seed' in effects.touch,false,'seed is not a Model');
});

test('only identity fields are copied, and Date components are encoded at the call',()=>{
 const effects=fresh();
 const at=new Date('2026-01-01T00:00:00.000Z');
 const record={at,title:'whole record'};
 effects.channel('c').moment.add(record);
 effects.touch.moment(record);
 at.setUTCFullYear(2030);
 record.title='changed';
 const pin={todo:'t',at:new Date('2026-02-01T00:00:00.000Z'),label:'x'};
 effects.channel('c').pin.remove(pin);
 effects.touch.pin(pin);
 pin.todo='other';
 pin.at.setUTCFullYear(2031);
 // A string DateTime component (a legacy wire identity) passes through for the engine to canonicalize.
 effects.touch.moment({at:'2026-03-01T00:00:00.000Z'});
 assert.deepEqual(effects.settlement(),{
  changes:[
   {model:'Moment',identity:{at:'2026-01-01T00:00:00.000Z'}},
   {model:'Pin',identity:{todo:'t',at:'2026-02-01T00:00:00.000Z'}},
   {model:'Moment',identity:{at:'2026-03-01T00:00:00.000Z'}},
  ],
  memberships:[
   add('c','Moment',{at:'2026-01-01T00:00:00.000Z'}),
   remove('c','Pin',{todo:'t',at:'2026-02-01T00:00:00.000Z'}),
  ],
 });
});

test('mixed calls take explicit references, including the generated constructors',()=>{
 const effects=fresh();
 const at=new Date('2026-01-01T00:00:00.000Z');
 const refs=[Todo({id:'A'}),Moment({at}),Pin({todo:'t',at}),{model:'Todo',identity:{id:'B',title:'extra'}}];
 effects.channel('mixed').add(refs);
 at.setUTCFullYear(2040);
 effects.channel('mixed').remove([Todo({id:'A'})]);
 effects.channel('mixed').add([]);
 effects.channel('mixed').remove([]);
 assert.deepEqual(effects.settlement(),{changes:[],memberships:[
  add('mixed','Todo',{id:'A'}),add('mixed','Moment',{at:'2026-01-01T00:00:00.000Z'}),
  add('mixed','Pin',{todo:'t',at:'2026-01-01T00:00:00.000Z'}),add('mixed','Todo',{id:'B'}),
  remove('mixed','Todo',{id:'A'}),
 ]});
});

test('missing, null, malformed and unknown references fail at the call',()=>{
 const effects=fresh();
 const channel=effects.channel('c');
 for(const identity of [undefined,null,'A',{},{id:null},{id:1},{title:'no id'}])
  assert.throws(()=>channel.todo.add(identity),/Todo/,JSON.stringify(identity));
 assert.throws(()=>effects.touch.todo({}),/Todo identity field id/);
 assert.throws(()=>effects.touch.moment({at:new Date('nope')}),/Moment identity field at/);
 assert.throws(()=>effects.touch.moment({at:'yesterday'}),/Moment identity field at/);
 assert.throws(()=>effects.touch.pin({todo:'t'}),/Pin identity field at/);
 assert.throws(()=>channel.add('x'),/array of record references/);
 assert.throws(()=>channel.add(Todo({id:'A'})),/array of record references/);
 assert.throws(()=>channel.add([null]),/record reference/);
 assert.throws(()=>channel.add([{model:'Nope',identity:{id:'x'}}]),/unknown Model Nope/);
 assert.throws(()=>channel.add([{model:'Todo'}]),/Todo identity/);
 // A raw identity names no Model, so a mixed call cannot place it.
 assert.throws(()=>channel.add([{id:'A'}]),/record reference/);
 assert.throws(()=>channel.remove([{id:'A'}]),/record reference/);
 assert.equal(effects.touch.nope,undefined);
 assert.equal(channel.nope,undefined);
 assert.deepEqual(effects.settlement(),empty,'nothing failed half-way into the declarations');
});

test('a mixed call with a later invalid element appends nothing, even when caught',()=>{
 const effects=fresh();
 const channel=effects.channel('c');
 assert.throws(()=>channel.add([Todo({id:'A'}),Todo({id:'B'}),{id:'C'}]),/record reference/);
 try{channel.remove([Todo({id:'A'}),{model:'Todo',identity:{}}]);}catch{}
 assert.deepEqual(effects.settlement(),empty);
 channel.add([Todo({id:'A'})]);
 assert.deepEqual(effects.settlement().memberships,[add('c','Todo',{id:'A'})]);
});

test('a Channel name must be a nonblank string; selecting one declares nothing',()=>{
 const effects=fresh();
 for(const name of ['','  ','\n',undefined,null,1,{}])
  assert.throws(()=>effects.channel(name),/Channel name/,String(name));
 effects.channel('selected');
 assert.deepEqual(effects.settlement(),empty);
});

test('handles are null-prototype dictionaries; __proto__, constructor and function member names are ordinary Models',()=>{
 const scalar={kind:'scalar',name:'string'};
 const effects=createEffects([
  {name:'__proto__',identity:['id'],fields:[{name:'id',type:scalar,nullable:false}]},
  {name:'Constructor',identity:['__proto__'],fields:[{name:'__proto__',type:scalar,nullable:false}]},
  {name:'ToString',identity:['id'],fields:[{name:'id',type:scalar,nullable:false}]},
  // A Channel handle is no function, so function members need no reservation either.
  ...['Name','Length','Bind','Apply','Call'].map(name=>({name,identity:['id'],fields:[{name:'id',type:scalar,nullable:false}]})),
 ]);
 assert.equal(Object.getPrototypeOf(effects.touch),null);
 const keys=['__proto__','constructor','toString','name','length','bind','apply','call'];
 assert.deepEqual(Object.keys(effects.touch),keys);
 const channel=effects.channel('c');
 assert.equal(Object.getPrototypeOf(channel),null);
 assert.deepEqual(Object.keys(channel),[...keys,'add','remove']);
 assert.equal(typeof channel,'object');
 channel.name.add({id:'n'});
 channel.length.remove({id:'l'});
 effects.touch.call({id:'k'});
 channel.__proto__.add({id:'p'});
 channel.constructor.remove({['__proto__']:'c'});
 effects.touch.toString({id:'s'});
 effects.touch.__proto__({id:'p'});
 const identity=Object.defineProperty({},'__proto__',{value:'c',enumerable:true});
 assert.deepEqual(effects.settlement(),{
  changes:[{model:'Call',identity:{id:'k'}},{model:'ToString',identity:{id:'s'}},{model:'__proto__',identity:{id:'p'}}],
  memberships:[add('c','Name',{id:'n'}),remove('c','Length',{id:'l'}),add('c','__proto__',{id:'p'}),remove('c','Constructor',identity)],
 });
 assert.equal(Object.hasOwn(effects.settlement().memberships[3].identity,'__proto__'),true);
 assert.equal({}.id,undefined,'Object.prototype is untouched');
 assert.equal(Object.getPrototypeOf(fresh().touch),null);
});

test('a malformed runtime config is refused: duplicate accessors and the reserved Channel keys',()=>{
 const model=name=>({name,identity:['id'],fields:[{name:'id',type:{kind:'scalar',name:'string'},nullable:false}]});
 assert.throws(()=>createEffects([model('Todo'),model('todo')]),/Models Todo and todo both generate the accessor todo/);
 assert.throws(()=>createEffects([model('Add')]),/Model Add generates the accessor add, which a Channel reserves/);
 assert.throws(()=>createEffects([model('remove')]),/Model remove generates the accessor remove, which a Channel reserves/);
 assert.throws(()=>createEffects([{name:'Todo',fields:[]}]),/Model Todo/);
 assert.throws(()=>createEffects([{name:'Todo',identity:['id'],fields:[]}]),/Todo identity field id/);
});

test('closing refuses every later declaration, including through escaped handles; settlement stays readable',()=>{
 const effects=fresh();
 const channel=effects.channel('c');
 const todo=channel.todo;
 const touch=effects.touch;
 channel.todo.add({id:'A'});
 touch.todo({id:'A'});
 effects.close();
 const closed=/closed/;
 assert.throws(()=>todo.add({id:'B'}),closed);
 assert.throws(()=>channel.todo.remove({id:'B'}),closed);
 assert.throws(()=>channel.add([]),closed);
 assert.throws(()=>channel.remove([Todo({id:'B'})]),closed);
 assert.throws(()=>touch.todo({id:'B'}),closed);
 assert.throws(()=>effects.channel('c'),closed);
 assert.throws(()=>effects.seed({model:'Todo',identity:{id:'B'}}),closed);
 const expected={changes:[{model:'Todo',identity:{id:'A'}}],memberships:[add('c','Todo',{id:'A'})]};
 const settled=effects.settlement();
 assert.deepEqual(settled,expected);
 // The answer is a copy of owned, frozen declarations.
 settled.changes.push({model:'Todo',identity:{id:'forged'}});
 assert.throws(()=>{settled.memberships[0].present=false;},TypeError);
 assert.throws(()=>{settled.memberships[0].identity.id='forged';},TypeError);
 assert.deepEqual(effects.settlement(),expected);
 effects.close();
 assert.deepEqual(effects.settlement(),expected,'closing twice is harmless');
});
