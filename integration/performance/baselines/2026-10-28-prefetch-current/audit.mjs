import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {readResult,compare,metrics} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const dir=dirname(fileURLToPath(import.meta.url)),r=await readResult(resolve(dir,'results.json.gz'));
assert.equal(r.completed,true);assert.equal(r.matrixCompleted,true);assert.equal(r.samples.length,4);
assert.deepEqual(r.samples.map(s=>s.bootstrapPrefetch),[false,true,true,false]);
assert.equal(r.experiment.runnerSha256,createHash('sha256').update(await readFile(resolve(dir,'runner.mjs'))).digest('hex'));
const runs=[];
for(const s of r.samples){
 assert.equal(s.status,'passed');assert.equal(s.serverVerified,true);assert.equal(s.result.verifiedRows,10000);
 assert.equal(s.serverVerification.rows,10000);assert.deepEqual(s.backendErrors,[]);assert.equal(s.serverDiagnosticsDropped,0);
 assert.equal(s.cleanupError,undefined);assert.equal(s.databaseCleanupError,undefined);
 assert.deepEqual(s.result.snapshots.map(x=>x.rows),Array.from({length:21},(_,i)=>i*500));
 const t=timeline(s),spans=t.traceEvents.filter(x=>x.ph==='X');
 const http=spans.filter(x=>x.name==='http.request'),apply=spans.filter(x=>x.name==='delivery.apply');
 assert.equal(s.serverDiagnostics.filter(x=>x.source==='node'&&x.name==='server.processDelivery05').length,20);
 assert.equal(apply.length,20);assert.equal(http.length,21);
 const overlap=http.slice(2).filter((h,i)=>h.ts<apply[i].ts+apply[i].dur).length;
 assert.equal(overlap,s.bootstrapPrefetch?19:0);
 runs.push({enabled:s.bootstrapPrefetch,repetition:s.repetition,metrics:metrics(s),successorsOverlappingPreviousApply:overlap,maxPlacementUncertaintyUs:t.clockAlignment.maxPlacementUncertaintyUs});
}
const evidence={commit:r.commit,dirty:r.dirty,build:r.build,machine:r.machine,compatibility:r.compatibility,experiment:r.experiment,runs,
 comparison:compare({...r,samples:r.samples.filter(s=>!s.bootstrapPrefetch)},{...r,samples:r.samples.filter(s=>s.bootstrapPrefetch)})};
await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');console.log(JSON.stringify(evidence.comparison,null,2));
