import test from 'node:test';
import assert from 'node:assert/strict';
import {Diagnostics} from '../../packages/backend/server/api/diagnostics.mts';
test('disabled diagnostics preserves the operation Promise without an async wrapper',()=>{
  const operation=Promise.resolve(42);
  assert.equal(new Diagnostics().span('test',{},()=>operation),operation);
});
test('backend receivers are optional and failures cannot replace an outcome',async()=>{
  for (const receiver of [undefined,{record(){throw Error('sink fault');}}]) {
    const trace=new Diagnostics(receiver);
    assert.equal(await trace.span('test',{},async()=>42),42);
    const expected=Error('original');
    await assert.rejects(trace.span('test',{},async()=>{throw expected;}),error=>error===expected);
  }
});
test('concurrent async spans retain their own parents and backend isolation',async()=>{
  const a=[],b=[],trace=new Diagnostics({record:r=>a.push(r)}),other=new Diagnostics({record:r=>b.push(r)});
  await Promise.all(['one','two'].map(name=>trace.span(name,{},async()=>{
    await new Promise(resolve=>setImmediate(resolve));
    await trace.span(name+'.child',{},async()=>{});
  })));
  await other.span('other',{},async()=>{});
  for (const name of ['one','two']) {
    const parent=a.find(r=>r.name===name),child=a.find(r=>r.name===name+'.child');
    assert.equal(child.parentSpanId,parent.spanId);
    assert.equal(child.attributes.operationId,parent.attributes.operationId);
    assert.ok(child.startUs>=parent.startUs);assert.ok(parent.durationUs>=child.durationUs);
  }
  assert.equal(b.length,1);assert.notEqual(a[0].clockDomain,b[0].clockDomain);
});
