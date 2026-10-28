import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {gunzipSync} from 'node:zlib';
import {metrics,summarize,statistics,validateRecordReports,serverSettlements} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const directory=dirname(fileURLToPath(import.meta.url));
const read=async name=>JSON.parse(gunzipSync(await readFile(resolve(directory,name))));
const before=await read('before.json.gz'),after=await read('after.json.gz');
assert.deepEqual(before.compatibility,after.compatibility,'timing, fixture and runtime conditions match');
assert.deepEqual(before.machine,after.machine,'machine, toolchain and PostgreSQL conditions match');
const result={contract:before.compatibility,before:{commit:before.commit,build:before.build,samples:before.samples.length},
 after:{commit:after.commit,build:after.build,samples:after.samples.length},cases:[]};
for(const [label,data]of [['before',before],['after',after]]) {
 assert.equal(data.completed,true,`${label} matrix completed`);
 assert.ok(data.samples.every(s=>s.status==='passed'),`${label} retains only passing correctness-verified samples`);
 const expected=data.parameters.samples*data.parameters.sizes.length*3;
 assert.equal(data.samples.length,expected,`${label} includes every initial, single-write and burst case`);
 assert.equal(data.compatibility.stageInstrumentation,3);
 // Validate bounded clock alignment, actual source positions, parents and no trace loss for every sample.
 let maxPlacementUncertaintyUs=0;
 for(const sample of data.samples) {
  validateRecordReports(sample);
  const trace=timeline(sample);
  maxPlacementUncertaintyUs=Math.max(maxPlacementUncertaintyUs,trace.clockAlignment.maxPlacementUncertaintyUs);
 }
 result[label].maxPlacementUncertaintyUs=maxPlacementUncertaintyUs;
 result[label].recordReports=data.samples.reduce((sum,s)=>sum+(s.result.recordReports?.length??0),0);
 const singles=data.samples.filter(s=>s.scenario==='live-sync'&&s.records===10000&&s.writes===1)
  .sort((a,b)=>metrics(a).bVisibleMs[0]-metrics(b).bVisibleMs[0]);
 const selected=singles[Math.floor(singles.length/2)];
 const trace=timeline(selected);
 const raw=selected.result.events[0];
 const records=selected.result.diagnostics;
 const b=records.filter(r=>r.client==='B'&&r.kind==='span');
 const stages={};
 for(const name of ['delivery.apply','watch.read','watch.convert','watch.compare','watch.snapshot','bridge.nativeDrain','bridge.decode','watch.enqueue']) {
  const matching=b.filter(r=>r.name===name);
  stages[name]={count:matching.length,totalMs:matching.reduce((sum,r)=>sum+r.durationUs,0)/1000,
   maxMs:Math.max(0,...matching.map(r=>r.durationUs))/1000};
 }
 const settlement=serverSettlements(selected)[0];
 const transaction=trace.traceEvents.find(r=>r.ph==='X'&&r.name==='server.transaction'
  &&r.args.spanId===settlement.transactionSpanId&&r.args.clockDomain===settlement.clockDomain);
 assert.ok(transaction,'named Mutation transaction is correlated to this write');
 const transactionRecords=trace.traceEvents.filter(r=>r.ph==='X'&&r.cat==='node'&&r.args.operationId===transaction.args.operationId);
 const backend={timeAxis:'milliseconds from shared scenario measurement start',
  transactionStartMs:transaction.ts/1000,transactionEndMs:(transaction.ts+transaction.dur)/1000,
  transactionDurationMs:transaction.dur/1000,frameworkSqlCalls:transactionRecords.filter(r=>r.name==='postgres.query').length,
  phases:transactionRecords.filter(r=>['host.applyStreamMembers','host.readTracking','host.targetPositions',
   'host.beginBatch','postgres.acquire','postgres.commit','handler.invoke'].includes(r.name))
   .map(r=>({name:r.name,startMs:r.ts/1000,endMs:(r.ts+r.dur)/1000,durationMs:r.dur/1000}))};
 result[label].selectedSingle={sampleIndex:data.samples.indexOf(selected),repetition:selected.repetition,writeStartUs:raw.startUs,backend,
  localCommitMs:(raw.localCommitUs-raw.startUs)/1000,settlementMs:(raw.settlementUs-raw.startUs)/1000,
  bVisibleMs:(raw.bVisibleUs-raw.startUs)/1000,maxPlacementUncertaintyUs:trace.clockAlignment.maxPlacementUncertaintyUs,
  stages,warning:'Source elapsed spans can overlap or nest. Totals do not form an end-to-end partition. watch.enqueue includes the synchronous SDK receiver after optimization.'};
 await writeFile(resolve(directory,`${label}-selected.trace.json`),JSON.stringify(trace)+'\n');
}
for(const group of summarize(before)) {
 const previous=before.samples.filter(s=>`${s.scenario}/${s.records}/${s.writes}`===group.key);
 const following=after.samples.filter(s=>`${s.scenario}/${s.records}/${s.writes}`===group.key);
 assert.ok(following.length,group.key);
 const names=Object.keys(metrics(previous[0]));
 const values={};
 for(const name of names) {
  const b=statistics(previous.flatMap(s=>metrics(s)[name]));
  const a=statistics(following.flatMap(s=>metrics(s)[name]));
  values[name]={before:b,after:a,medianRatio:a.median/b.median};
 }
 result.cases.push({key:group.key,metrics:values});
}
await writeFile(resolve(directory,'evidence.json'),JSON.stringify(result,null,2)+'\n');
console.log(JSON.stringify({before:result.before,after:result.after,cases:result.cases.map(c=>({key:c.key,
 ...(c.metrics.bVisibleMs?{bVisibleMs:c.metrics.bVisibleMs}:{}),allVisibleMs:c.metrics.allVisibleMs}))},null,2));
