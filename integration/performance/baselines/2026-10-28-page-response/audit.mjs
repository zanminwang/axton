import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {readResult,metrics,statistics} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const dir=dirname(fileURLToPath(import.meta.url));
const names=['before-1','after-1','after-2','before-2'];
const runnerHash=createHash('sha256').update(await readFile(resolve(dir,'runner.mjs'))).digest('hex');
const runs=[];
for(const name of names){
 const r=await readResult(resolve(dir,name+'.json.gz'));
 assert.equal(r.completed,true);assert.equal(r.samples.length,1);
 assert.equal(r.experiment.runnerSha256,runnerHash);
 const s=r.samples[0];
 assert.equal(s.status,'passed');assert.equal(s.bootstrapPrefetch,true);
 assert.equal(s.serverVerified,true);assert.equal(s.result.verifiedRows,10000);
 assert.equal(s.serverVerification.rows,10000);assert.deepEqual(s.backendErrors,[]);
 assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.cleanupError,undefined);
 assert.equal(s.databaseCleanupError,undefined);
 assert.deepEqual(s.result.snapshots.map(x=>x.rows),Array.from({length:21},(_,i)=>i*500));
 assert.equal(s.serverDiagnostics.filter(x=>x.source==='node'&&x.name==='server.processDelivery05').length,20);
 const stages={};
 for(const d of s.serverDiagnostics)if(d.kind==='span'&&d.name.startsWith('delivery.')){
  const stage=stages[d.name]??={count:0,ms:0};stage.count++;stage.ms+=d.durationUs/1000;
 }
 const t=timeline(s),spans=t.traceEvents.filter(x=>x.ph==='X');
 const http=spans.filter(x=>x.name==='http.request'),apply=spans.filter(x=>x.name==='delivery.apply');
 assert.equal(http.length,21);assert.equal(apply.length,20);
 const overlap=http.slice(2).filter((h,i)=>h.ts<apply[i].ts+apply[i].dur).length;
 assert.equal(overlap,19);
 runs.push({name,build:r.build,metrics:metrics(s),stages,overlap,
  maxPlacementUncertaintyUs:t.clockAlignment.maxPlacementUncertaintyUs});
}
const serverArtifact='bindings/node/axton-node.node';
for(const path of Object.keys(runs[0].build.artifacts).filter(p=>p!==serverArtifact))
 assert.ok(runs.every(r=>r.build.artifacts[path]===runs[0].build.artifacts[path]),'client artifacts held constant');
assert.equal(runs[0].build.artifacts[serverArtifact],runs[3].build.artifacts[serverArtifact]);
assert.equal(runs[1].build.artifacts[serverArtifact],runs[2].build.artifacts[serverArtifact]);
assert.notEqual(runs[0].build.artifacts[serverArtifact],runs[1].build.artifacts[serverArtifact]);
const summary={};
for(const variant of ['before','after']){
 const selected=runs.filter(r=>r.name.startsWith(variant));
 summary[variant]={metrics:Object.fromEntries(Object.keys(selected[0].metrics).map(k=>[k,statistics(selected.flatMap(r=>r.metrics[k]))])),
  stages:Object.fromEntries(Object.keys(selected[0].stages).map(k=>[k,statistics(selected.map(r=>r.stages[k].ms))]))};
}
const evidence={order:names,runnerSha256:runnerHash,runs,summary};
await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify(summary,null,2));
