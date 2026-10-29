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
 assert.equal(r.experiment.clientVariant,name.split('-')[0]);
 assert.equal(s.serverVerified,true);assert.equal(s.result.verifiedRows,10000);
 assert.equal(s.serverVerification.rows,10000);assert.deepEqual(s.backendErrors,[]);
 assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.cleanupError,undefined);
 assert.equal(s.databaseCleanupError,undefined);
 assert.deepEqual(s.result.snapshots.map(x=>x.rows),Array.from({length:21},(_,i)=>i*500));
 assert.equal(s.serverDiagnostics.filter(x=>x.source==='node'&&x.name==='server.processDelivery05').length,20);
 const stages={};
 for(const d of [...s.serverDiagnostics,...s.result.diagnostics])if(d.kind==='span'&&(d.name.startsWith('delivery.')||['watch.query','sqlite.commit'].includes(d.name))){
  const key=d.source+'/'+d.name;const stage=stages[key]??={count:0,ms:0};stage.count++;stage.ms+=d.durationUs/1000;
 }
 const applies=s.result.diagnostics.filter(d=>d.kind==='span'&&d.name==='delivery.apply');
 const commits=s.result.diagnostics.filter(d=>d.kind==='span'&&d.name==='sqlite.commit'&&applies.some(a=>a.clockDomain===d.clockDomain&&a.startUs<=d.startUs&&a.startUs+a.durationUs>=d.startUs+d.durationUs));
 assert.equal(applies.length,20);assert.equal(commits.length,20);
 stages['rust/delivery.apply.commit']={count:commits.length,ms:commits.reduce((n,d)=>n+d.durationUs/1000,0)};
 for(const key of ['keys','authority','authority.clear','authority.install','reconcile','rebuild'])assert.equal(stages['rust/delivery.apply.'+key].count,20);
 const t=timeline(s),spans=t.traceEvents.filter(x=>x.ph==='X');
 const http=spans.filter(x=>x.name==='http.request'),apply=spans.filter(x=>x.name==='delivery.apply');
 assert.equal(http.length,21);assert.equal(apply.length,20);
 const responseToApplyGapMs=http.slice(1).map((h,i)=>(apply[i].ts-h.ts-h.dur)/1000);
 assert.ok(responseToApplyGapMs.every(x=>x>=0));
 const httpOverlappingApplyMs=apply.reduce((n,a)=>n+http.reduce((m,h)=>m+Math.max(0,Math.min(a.ts+a.dur,h.ts+h.dur)-Math.max(a.ts,h.ts))/1000,0),0);
 const overlap=http.slice(2).filter((h,i)=>h.ts<apply[i].ts+apply[i].dur).length;
 assert.equal(overlap,19);
 runs.push({name,build:r.build,metrics:metrics(s),stages,overlap,responseToApplyGapMs,httpOverlappingApplyMs,
  maxPlacementUncertaintyUs:t.clockAlignment.maxPlacementUncertaintyUs});
}
const serverArtifact='target/release/libaxton_dart.dylib';
for(const path of Object.keys(runs[0].build.artifacts).filter(p=>p!==serverArtifact))
 assert.ok(runs.every(r=>r.build.artifacts[path]===runs[0].build.artifacts[path]),'backend and AOT artifacts held constant');
assert.equal(runs[0].build.artifacts[serverArtifact],runs[3].build.artifacts[serverArtifact]);
assert.equal(runs[1].build.artifacts[serverArtifact],runs[2].build.artifacts[serverArtifact]);
assert.notEqual(runs[0].build.artifacts[serverArtifact],runs[1].build.artifacts[serverArtifact]);
const summary={};
for(const variant of ['before','after']){
 const selected=runs.filter(r=>r.name.startsWith(variant));
 summary[variant]={responseToApplyGapMs:statistics(selected.flatMap(r=>r.responseToApplyGapMs)),httpOverlappingApplyMs:statistics(selected.map(r=>r.httpOverlappingApplyMs)),metrics:Object.fromEntries(Object.keys(selected[0].metrics).map(k=>[k,statistics(selected.flatMap(r=>r.metrics[k]))])),
  stages:Object.fromEntries(Object.keys(selected[0].stages).map(k=>[k,statistics(selected.map(r=>r.stages[k].ms))]))};
}
const evidence={order:names,runnerSha256:runnerHash,runs,summary};
await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify(summary,null,2));
