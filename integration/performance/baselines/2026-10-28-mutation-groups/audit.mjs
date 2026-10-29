import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {readResult,metrics,statistics,validateRecordReports,compare} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const dir=dirname(fileURLToPath(import.meta.url));
const raw=await readResult(resolve(dir,'results.json.gz'));
assert.equal(raw.completed,true);assert.equal(raw.matrixCompleted,true);assert.equal(raw.samples.length,6);
assert.equal(raw.experiment.runnerSha256,createHash('sha256').update(await readFile(resolve(dir,'runner.mjs'))).digest('hex'));
assert.deepEqual(raw.samples.map(s=>s.groupSize),[1,8,8,1,1,8]);
const evidence={build:raw.build,commit:raw.commit,dirty:raw.dirty,machine:raw.machine,compatibility:raw.compatibility,experiment:raw.experiment,runs:[],conditions:{}};
for(const s of raw.samples){
 assert.equal(s.status,'passed');assert.equal(s.records,1000);assert.equal(s.writes,100);
 assert.equal(s.serverVerified,true);assert.equal(s.result.verifiedRows,1100);assert.equal(s.serverVerification.rows,1100);
 assert.deepEqual(s.backendErrors,[]);assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.cleanupError,undefined);assert.equal(s.databaseCleanupError,undefined);
 validateRecordReports(s);const trace=timeline(s);
 const spans=s.serverDiagnostics.filter(r=>r.kind==='span'&&r.source==='node');
 const ops=new Set(spans.filter(r=>r.name==='host.handleAction').map(r=>r.attributes.operationId));
 const count=name=>spans.filter(r=>r.name===name).length;
 const mutationTransactions=spans.filter(r=>r.name==='server.transaction'&&ops.has(r.attributes.operationId));
 if(s.groupSize===1)assert.equal(mutationTransactions.length,100);else assert.ok(mutationTransactions.length<100);
 evidence.runs.push({groupSize:s.groupSize,repetition:s.repetition,metrics:metrics(s),mutationTransactions:mutationTransactions.length,
   mutationTransactionTotalMs:mutationTransactions.reduce((n,r)=>n+r.durationUs/1000,0),
   frameworkSql:count('postgres.query'),livePlans:count('server.processLive05'),
   recordReports:s.result.recordReports.length,maxPlacementUncertaintyUs:trace.clockAlignment.maxPlacementUncertaintyUs});
}
for(const size of [1,8]){
 const rows=evidence.runs.filter(r=>r.groupSize===size);
 evidence.conditions[size]={metrics:Object.fromEntries(Object.keys(rows[0].metrics).filter(n=>n!=='serverTransactionMs').map(n=>[n,statistics(rows.flatMap(r=>r.metrics[n]))])),
  mutationTransactions:statistics(rows.map(r=>r.mutationTransactions)),frameworkSql:statistics(rows.map(r=>r.frameworkSql)),livePlans:statistics(rows.map(r=>r.livePlans)),recordReports:rows.reduce((n,r)=>n+r.recordReports,0)};
}
evidence.comparison=compare({...raw,samples:raw.samples.filter(s=>s.groupSize===1)},{...raw,samples:raw.samples.filter(s=>s.groupSize===8)}).filter(r=>r.metric!=='serverTransactionMs');
evidence.pairs=[];
for(let i=0;i<6;i+=2){const pair=evidence.runs.slice(i,i+2),a=pair.find(r=>r.groupSize===1),b=pair.find(r=>r.groupSize===8);evidence.pairs.push({order:pair.map(r=>r.groupSize),allVisibleDifferenceMs:b.metrics.allVisibleMs[0]-a.metrics.allVisibleMs[0]});}
await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify({conditions:evidence.conditions,pairs:evidence.pairs},null,2));
