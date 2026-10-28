import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {compare,metrics,readResult,statistics,validateRecordReports} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const directory=dirname(fileURLToPath(import.meta.url));
const raw=await readResult(resolve(directory,'interleaved-live.json.gz'));
const original=JSON.parse(await readFile(resolve(directory,'evidence.json'),'utf8'));
assert.equal(raw.completed,true);
assert.equal(raw.matrixCompleted,true);
assert.equal(raw.dirty,false);
assert.deepEqual(raw.build,original.build,'reuse the exact original release artifacts');
assert.deepEqual(raw.compatibility,original.compatibility);
assert.deepEqual(raw.machine,original.machine);
assert.equal(raw.samples.length,10);
const sequence=[false,true,true,false,false,true,true,false,false,true];
assert.deepEqual(raw.experiment.sequence,sequence);
assert.equal(createHash('sha256').update(await readFile(resolve(directory,'interleaved-runner.mjs'))).digest('hex'),raw.experiment.runnerSha256);
const split=enabled=>({...raw,samples:raw.samples.filter(s=>s.bootstrapPrefetch===enabled)});
const evidence={commit:raw.commit,experiment:raw.experiment,comparison:compare(split(false),split(true)),runs:[],conditions:{}};
let maxPlacementUncertaintyUs=0;
for(const [index,s]of raw.samples.entries()){
 assert.equal(s.sequenceIndex,index);
 assert.equal(s.bootstrapPrefetch,sequence[index]);
 assert.equal(s.status,'passed');
 assert.equal(s.serverVerified,true);
 assert.equal(s.result.verifiedRows,10001);
 assert.equal(s.scenario,'live-sync');
 assert.equal(s.writes,1);
 assert.equal(s.backendErrors.length,0);
 validateRecordReports(s);
 const trace=timeline(s),spans=trace.traceEvents.filter(e=>e.ph==='X');
 maxPlacementUncertaintyUs=Math.max(maxPlacementUncertaintyUs,trace.clockAlignment.maxPlacementUncertaintyUs);
 const lanes=Object.fromEntries(trace.traceEvents.filter(e=>e.name==='thread_name').map(e=>[e.tid,e.args.name]));
 const applies=spans.filter(e=>e.name==='delivery.apply'&&lanes[e.tid].startsWith('B '));
 assert.equal(applies.length,1);
 const operation=spans.find(e=>e.name==='server.processBatchMember');
 const transaction=spans.find(e=>e.name==='server.transaction'&&e.args.operationId===operation.args.operationId);
 const zero=trace.traceEvents.find(e=>e.name==='A write starts').ts;
 const visible=trace.traceEvents.find(e=>e.name==='B watch visible').ts;
 const calls=s.serverDiagnostics.filter(r=>r.kind==='span'&&r.source==='node'&&r.name==='postgres.query').length;
 const pulls=spans.filter(e=>e.name==='server.processDelivery05').length;
 assert.equal(calls,41);
 assert.equal(pulls,0,'no ordinary repair is reintroduced');
 evidence.runs.push({sequenceIndex:index,enabled:s.bootstrapPrefetch,repetition:s.repetition,metrics:metrics(s),
  bApplyMs:applies[0].dur/1000,transactionStartMs:(transaction.ts-zero)/1000,
  transactionEndMs:(transaction.ts+transaction.dur-zero)/1000,
  commitToBVisibleMs:(visible-transaction.ts-transaction.dur)/1000,
  frameworkSql:calls,ordinaryPulls:pulls});
}
for(const enabled of [false,true]){
 const label=enabled?'on':'off',samples=split(enabled).samples;
 assert.equal(samples.length,5);
 evidence.conditions[label]={metrics:Object.fromEntries(Object.keys(metrics(samples[0])).map(name=>
  [name,statistics(samples.flatMap(s=>metrics(s)[name]))])),
  bApplyMs:statistics(evidence.runs.filter(r=>r.enabled===enabled).map(r=>r.bApplyMs))};
 const selected=[...samples].sort((a,b)=>metrics(a).bVisibleMs[0]-metrics(b).bVisibleMs[0])[2];
 await writeFile(resolve(directory,`interleaved-${label}-single.trace.json`),JSON.stringify(timeline(selected))+'\n');
}
evidence.pairs=[];
for(let i=0;i<evidence.runs.length;i+=2){
 const pair=evidence.runs.slice(i,i+2),off=pair.find(r=>!r.enabled),on=pair.find(r=>r.enabled);
 const difference=name=>on.metrics[name][0]-off.metrics[name][0];
 evidence.pairs.push({pair:i/2+1,order:pair.map(r=>r.enabled?'on':'off'),
  bVisibleDifferenceMs:difference('bVisibleMs'),serverTransactionDifferenceMs:difference('serverTransactionMs')});
}
evidence.pairedBVisibleDifferenceMs=statistics(evidence.pairs.map(p=>p.bVisibleDifferenceMs));
evidence.maxPlacementUncertaintyUs=maxPlacementUncertaintyUs;
await writeFile(resolve(directory,'interleaved-evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify(evidence,null,2));
