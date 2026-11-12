import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createClient } from './api/runtime.mts';
import { Transaction } from './api/transaction.mts';
const native = createRequire(import.meta.url)('../../../bindings/node/axton-node.node');
const Client = createClient(native, Transaction, () => ({ push: async () => { throw Error('offline'); }, open() {} }));
const connection = { url:'http://127.0.0.1:1', token:'offline' };
const schema = JSON.parse(await readFile(new URL('../../../integration/v05-sdk/schema.json',import.meta.url),'utf8'));
test('captured transaction cannot join a different client callback', async () => {
  const directory=await mkdtemp(join(tmpdir(),'axton-foreign04-'));
  const a=await Client.open({path:join(directory,'a'),schema,stream:'User:alice',connection});
  const b=await Client.open({path:join(directory,'b'),schema,stream:'User:alice',connection});
  try {
    await assert.rejects(a.transaction(async txA => {
      await b.transaction(async () => {
        await assert.rejects(txA.submitMutation('Publish',1,{entry:{id:'foreign',text:'bad'},call:'legal'},value=>value), /foreign transaction scope/);
      });
    }), /foreign transaction scope/);
    assert.equal(await a.read('Entry',{id:'foreign'}),null);
    assert.equal(await b.read('Entry',{id:'foreign'}),null);
  } finally { await a.close(); await b.close(); await rm(directory,{recursive:true,force:true}); }
});
test('captured empty savepoint cannot join another Store callback',async()=>{
 const directory=await mkdtemp(join(tmpdir(),'axton-savepoint-foreign04-'));
 const a=await Client.open({path:join(directory,'a'),schema,stream:'User:alice',connection});
 const b=await Client.open({path:join(directory,'b'),schema,stream:'User:alice',connection});
 let ran=false;
 try {
  await assert.rejects(a.transaction(async txA=>{
   await b.transaction(async()=>{
    await assert.rejects(txA.savepoint(async()=>{ran=true;return 42;}),/foreign transaction scope/);
   });
  }),/foreign transaction scope/);
  assert.equal(ran,false);
 } finally {await a.close();await b.close();await rm(directory,{recursive:true,force:true});}
});
