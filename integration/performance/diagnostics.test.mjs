import test from 'node:test';
import assert from 'node:assert/strict';
import {Diagnostics} from '../../packages/backend/server/api/diagnostics.mts';
test('backend clock metadata brackets its origin in the process monotonic clock',()=>{
  const records=[], before=Number(process.hrtime.bigint()/1000n);
  new Diagnostics({record:r=>records.push(r)});
  const after=Number(process.hrtime.bigint()/1000n),anchor=records[0];
  assert.equal(anchor.kind,'clock');assert.equal(anchor.attributes.referenceClockDomain,`node:${process.pid}`);
  assert.ok(anchor.attributes.offsetLowerUs>=before&&anchor.attributes.offsetUpperUs<=after+1);
});
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
  assert.equal(b.filter(r=>r.kind==='span').length,1);assert.notEqual(a[0].clockDomain,b[0].clockDomain);
});

test('synchronous diagnostic spans retain parentage, results and original errors', async () => {
  const records=[];
  const d=new Diagnostics({record:r=>records.push(r)});
  const error=new Error('encode refused');
  await d.span('parent',{},async()=>{
    assert.equal(d.syncSpan('convert',{},()=>42),42);
    assert.throws(()=>d.syncSpan('bad',{},()=>{throw error;}),e=>e===error);
  });
  const parent=records.find(r=>r.name==='parent');
  assert.equal(records.find(r=>r.name==='convert').parentSpanId,parent.spanId);
  assert.equal(records.find(r=>r.name==='bad').attributes.outcome,'error');
});
