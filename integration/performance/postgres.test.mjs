import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {poolLifecycle} from './support/postgres.mjs';
test('database teardown waits for physical sockets after Pool.end resolves',async()=>{
  const pool=new EventEmitter();pool.end=()=>Promise.resolve();
  const errors=[],close=poolLifecycle(pool,error=>errors.push(error));
  const client=new EventEmitter();pool.emit('connect',client);
  let closed=false;const closing=close().then(()=>{closed=true;});
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(closed,false,'pool resolution is not a socket-close barrier');
  client.emit('end');await closing;assert.equal(closed,true);
  await close();
  const fault=Error('pool fault');pool.emit('error',fault);assert.deepEqual(errors,[fault]);
});
