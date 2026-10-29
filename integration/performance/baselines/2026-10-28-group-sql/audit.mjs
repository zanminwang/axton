import assert from 'node:assert/strict';
import{readFile,writeFile}from'node:fs/promises';import{createHash}from'node:crypto';
import{dirname,resolve}from'node:path';import{fileURLToPath}from'node:url';
import{readResult,metrics,statistics,validateRecordReports}from'../../report.mjs';
import{timeline}from'../../timeline.mjs';
const dir=dirname(fileURLToPath(import.meta.url)),names=['before-1','after-1','after-2','before-2'];
const runnerSha256=createHash('sha256').update(await readFile(resolve(dir,'runner.mjs'))).digest('hex');
const runs=[];
for(const name of names){
 const r=await readResult(resolve(dir,name+'.json.gz'));assert.equal(r.completed,true);assert.equal(r.samples.length,1);
 assert.equal(r.experiment.variant,name.split('-')[0]);assert.equal(r.experiment.runnerSha256,runnerSha256);
 const s=r.samples[0];assert.equal(s.status,'passed');assert.equal(s.scenario,'live-sync');assert.equal(s.records,10000);assert.equal(s.writes,100);
 assert.equal(s.serverVerified,true);assert.equal(s.result.verifiedRows,10100);assert.equal(s.serverVerification.rows,10100);assert.equal(s.result.accepted,100);
 assert.deepEqual(s.backendErrors,[]);assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.cleanupError,undefined);assert.equal(s.databaseCleanupError,undefined);
 assert.equal(s.result.events.length,100);assert.equal(s.serverSettlements.length,100);validateRecordReports(s);
 assert.ok(s.result.events.every(e=>e.accepted&&Number.isFinite(e.settlementUs)&&Number.isFinite(e.bVisibleUs)));
 const spans=s.serverDiagnostics.filter(d=>d.kind==='span'),groups=spans.filter(d=>d.name==='server.processBatchGroup');
 assert.equal(groups.length,14);const ids=new Set(groups.map(d=>d.attributes.operationId));
 const queries=spans.filter(d=>d.name==='postgres.query'),mutationQueries=queries.filter(d=>ids.has(d.attributes.operationId));
 const count=n=>spans.filter(d=>d.name===n).length;
 const optimized=name.startsWith('after');
 assert.equal(count('host.claimStore'),optimized?14:100);
 assert.equal(count('host.saveResult'),optimized?0:100);assert.equal(count('host.saveResults'),optimized?14:0);
 assert.equal(queries.length,optimized?1371:1543);assert.equal(mutationQueries.length,optimized?1131:1303);
 runs.push({name,build:r.build,metrics:metrics(s),mutationGroups:groups.length,frameworkQueries:queries.length,
 mutationFrameworkQueries:mutationQueries.length,frameworkQueryMs:queries.reduce((n,d)=>n+d.durationUs/1000,0),
 mutationFrameworkQueryMs:mutationQueries.reduce((n,d)=>n+d.durationUs/1000,0),groupMs:groups.reduce((n,d)=>n+d.durationUs/1000,0),
 storeClaims:count('host.claimStore'),resultWrites:count(optimized?'host.saveResults':'host.saveResult'),
 maxPlacementUncertaintyUs:timeline(s).clockAlignment.maxPlacementUncertaintyUs});
}
const server='bindings/node/axton-node.node';
for(const path of Object.keys(runs[0].build.artifacts).filter(p=>p!==server))assert.ok(runs.every(r=>r.build.artifacts[path]===runs[0].build.artifacts[path]));
assert.ok(runs.every(r=>r.build.sourceSha256===runs[0].build.sourceSha256));
assert.equal(runs[0].build.artifacts[server],runs[3].build.artifacts[server]);assert.equal(runs[1].build.artifacts[server],runs[2].build.artifacts[server]);
assert.notEqual(runs[0].build.artifacts[server],runs[1].build.artifacts[server]);
const previous=JSON.parse(await readFile(resolve(dir,'../2026-10-28-performance-summary/evidence.json'),'utf8'));
assert.equal(runs[0].build.artifacts[server],previous.build.artifacts[server]);
const summary={};for(const variant of ['before','after']){const selected=runs.filter(r=>r.name.startsWith(variant));
 summary[variant]={metrics:Object.fromEntries(Object.keys(selected[0].metrics).map(k=>[k,statistics(selected.flatMap(r=>r.metrics[k]))])),
 stages:Object.fromEntries(['frameworkQueries','mutationFrameworkQueries','frameworkQueryMs','mutationFrameworkQueryMs','groupMs','storeClaims','resultWrites'].map(k=>[k,statistics(selected.map(r=>r[k]))]))};}
const evidence={order:names,runnerSha256,runs,summary};await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify(summary,null,2));
