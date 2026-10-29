import assert from 'node:assert/strict';
import {writeFile,readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {readResult,summarize,validateRecordReports} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const dir=dirname(fileURLToPath(import.meta.url));
const r=await readResult(resolve(dir,'results.json.gz'));
assert.equal(r.completed,true);assert.equal(r.matrixCompleted,true);assert.equal(r.samples.length,6);
assert.deepEqual(r.parameters.sizes,[10000]);assert.equal(r.parameters.samples,2);
const runs=[];
for(const s of r.samples){
 assert.equal(s.status,'passed');assert.equal(s.serverVerified,true);assert.deepEqual(s.backendErrors,[]);
 assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.cleanupError,undefined);assert.equal(s.databaseCleanupError,undefined);
 assert.equal(s.result.verifiedRows,10000+s.writes);assert.equal(s.serverVerification.rows,10000+s.writes);
 validateRecordReports(s);
 const t=timeline(s),spans=t.traceEvents.filter(x=>x.ph==='X');
 const http=spans.filter(x=>x.name==='http.request'),apply=spans.filter(x=>x.name==='delivery.apply');
 const queries=s.serverDiagnostics.filter(d=>d.name==='postgres.query'&&d.kind==='span');
 const groups=s.serverDiagnostics.filter(d=>d.name==='server.processBatchGroup'&&d.kind==='span');
 const groupIds=new Set(groups.map(d=>d.attributes.operationId));
 const mutationQueries=queries.filter(d=>groupIds.has(d.attributes.operationId));
 const entry={mutationFrameworkQueries:mutationQueries.length,mutationFrameworkQueryMs:mutationQueries.reduce((n,d)=>n+d.durationUs/1000,0),scenario:s.scenario,writes:s.writes,repetition:s.repetition,frameworkQueries:queries.length,
 frameworkQueryMs:queries.reduce((n,d)=>n+d.durationUs/1000,0),mutationGroups:groups.length,
 maxPlacementUncertaintyUs:t.clockAlignment.maxPlacementUncertaintyUs};
 if(s.scenario==='initial-sync'){
  assert.deepEqual(s.result.snapshots.map(x=>x.rows),Array.from({length:21},(_,i)=>i*500));
  assert.equal(http.length,21);assert.equal(apply.length,20);
  entry.successorsOverlappingPreviousApply=http.slice(2).filter((h,i)=>h.ts<apply[i].ts+apply[i].dur).length;
  assert.equal(entry.successorsOverlappingPreviousApply,19);
  entry.responseToApplyGapMs=http.slice(1).map((h,i)=>(apply[i].ts-h.ts-h.dur)/1000);
 }else{
  assert.equal(s.result.accepted,s.writes);assert.equal(s.result.events.length,s.writes);
  assert.equal(s.serverSettlements.length,s.writes);
  assert.ok(s.result.events.every(e=>e.accepted&&Number.isFinite(e.settlementUs)&&Number.isFinite(e.bVisibleUs)));
 }
 runs.push(entry);
}
const evidence={commit:r.commit,dirty:r.dirty,build:r.build,machine:r.machine,compatibility:r.compatibility,
 parameters:r.parameters,runnerSha256:createHash('sha256').update(await readFile(resolve(dir,'runner.mjs'))).digest('hex'),
 runs,summary:summarize(r)};
assert.deepEqual(evidence.summary,r.summary);
await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify(evidence.summary.map(g=>({key:g.key,metrics:g.metrics})),null,2));
