import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {caseKey,compare,metrics,readResult,serverSettlements,statistics,validateRecordReports} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';

const directory=dirname(fileURLToPath(import.meta.url));
const paths={before:'../2026-10-27-model-watch-delivery/after.json.gz',after:'after.json.gz'};
const data=Object.fromEntries(await Promise.all(Object.entries(paths).map(async([label,path])=>
  [label,await readResult(resolve(directory,path))])));
const comparison=compare(data.before,data.after);
const evidence={contract:data.after.compatibility,machine:data.after.machine,comparison,cases:[]};
const operations=['server.processBatchMember','server.processLive05','server.processDelivery05','server.processMaterialization05'];

for(const [label,result]of Object.entries(data)) {
  assert.equal(result.completed,true,`${label}: complete passing matrix`);
  assert.equal(result.matrixCompleted,true,`${label}: every planned sample attempted`);
  assert.equal(result.compatibility.stageInstrumentation,3);
  assert.equal(result.parameters.samples,5);
  assert.deepEqual(result.parameters.sizes,[100,1000,10000]);
  assert.equal(result.parameters.writes,100);
  assert.equal(result.samples.length,45);
  let maxPlacementUncertaintyUs=0;
  for(const sample of result.samples) {
    assert.equal(sample.status,'passed');
    assert.equal(sample.serverVerified,true);
    assert.equal(sample.backendErrors.length,0);
    validateRecordReports(sample);
    const trace=timeline(sample);
    maxPlacementUncertaintyUs=Math.max(maxPlacementUncertaintyUs,trace.clockAlignment.maxPlacementUncertaintyUs);
  }
  const selected=result.samples.filter(s=>caseKey(s)==='live-sync/10000/1')
    .sort((a,b)=>metrics(a).bVisibleMs[0]-metrics(b).bVisibleMs[0])[2];
  const trace=timeline(selected),raw=selected.result.events[0],zero=raw.startUs;
  const spans=trace.traceEvents.filter(e=>e.ph==='X');
  const settlement=serverSettlements(selected)[0];
  const transaction=spans.find(e=>e.name==='server.transaction'&&e.args.spanId===settlement.transactionSpanId
    &&e.args.clockDomain===settlement.clockDomain);
  assert.ok(transaction,'named Mutation transaction is correlated to the selected write');
  const stageNames=['delivery.apply','watch.query','watch.snapshot','bridge.nativeDrain','bridge.decode','watch.enqueue'];
  const stages=Object.fromEntries(stageNames.map(name=>{
    const records=selected.result.diagnostics.filter(r=>r.client==='B'&&r.kind==='span'&&r.name===name);
    return [name,{count:records.length,totalMs:records.reduce((n,r)=>n+r.durationUs/1000,0),
      maxMs:Math.max(0,...records.map(r=>r.durationUs/1000))}];
  }));
  const transactions=spans.filter(e=>e.name==='server.transaction').map(tx=>{
    const related=spans.filter(e=>e.args.operationId===tx.args.operationId);
    return {operationId:tx.args.operationId,startMs:(tx.ts-zero)/1000,durationMs:tx.dur/1000,
      operation:related.find(e=>operations.includes(e.name))?.name??'unknown',
      frameworkSqlCalls:related.filter(e=>e.cat==='node'&&e.name==='postgres.query').length};
  }).sort((a,b)=>a.startMs-b.startMs);
  evidence[label]={path:paths[label],commit:result.commit,dirty:result.dirty,build:result.build,
    samples:result.samples.length,maxPlacementUncertaintyUs,
    recordReports:result.samples.reduce((n,s)=>n+(s.result.recordReports?.length??0),0),
    selectedSingle:{sampleIndex:result.samples.indexOf(selected),repetition:selected.repetition,writeStartUs:zero,
      localCommitMs:(raw.localCommitUs-zero)/1000,settlementMs:(raw.settlementUs-zero)/1000,
      bVisibleMs:(raw.bVisibleUs-zero)/1000,
      maxPlacementUncertaintyUs:trace.clockAlignment.maxPlacementUncertaintyUs,
      backend:{transactionStartMs:(transaction.ts-zero)/1000,transactionDurationMs:transaction.dur/1000,
        frameworkSqlCalls:spans.filter(e=>e.cat==='node'&&e.name==='postgres.query'
          &&e.args.operationId===transaction.args.operationId).length},transactions,stages}};
  await writeFile(resolve(directory,`${label}-selected.trace.json`),JSON.stringify(trace)+'\n');
}
for(const key of [...new Set(data.before.samples.map(caseKey))]) {
  const entry={key};
  for(const [label,result]of Object.entries(data)) {
    const samples=result.samples.filter(s=>caseKey(s)===key);
    const counts=samples.map(sample=>{
      const spans=sample.serverDiagnostics.filter(r=>r.kind==='span');
      return {frameworkSql:spans.filter(r=>r.source==='node'&&r.name==='postgres.query').length,
        transactions:spans.filter(r=>r.source==='node'&&r.name==='server.transaction').length,
        ...Object.fromEntries(operations.map(name=>[name,spans.filter(r=>r.name===name).length]))};
    });
    if(samples[0].scenario==='live-sync'&&samples[0].writes===1) {
      for(const count of counts) {
        assert.equal(count['server.processDelivery05'],label==='before'?1:0,'single-write ordinary Pull count');
        assert.equal(count.transactions,label==='before'?5:4,'single-write transaction count');
        assert.equal(count.frameworkSql,label==='before'?47:41,'single-write framework SQL count');
      }
    }
    entry[label]={metrics:Object.fromEntries(Object.keys(metrics(samples[0])).map(name=>
      [name,statistics(samples.flatMap(s=>metrics(s)[name]))])),
      counts:Object.fromEntries(Object.keys(counts[0]).map(name=>[name,statistics(counts.map(c=>c[name]))]))};
  }
  evidence.cases.push(entry);
}
await writeFile(resolve(directory,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify({before:evidence.before,after:evidence.after,
  comparison:comparison.filter(row=>['bVisibleMs','settlementMs','allVisibleMs'].includes(row.metric))},null,2));
