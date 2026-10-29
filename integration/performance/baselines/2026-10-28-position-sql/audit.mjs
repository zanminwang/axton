import assert from 'node:assert/strict';
import{readFile,writeFile}from'node:fs/promises';import{createHash}from'node:crypto';import{dirname,resolve}from'node:path';import{fileURLToPath}from'node:url';
import{readResult,metrics,statistics,validateRecordReports}from'../../report.mjs';import{timeline}from'../../timeline.mjs';
const dir=dirname(fileURLToPath(import.meta.url)),names=['before-1','after-1','after-2','before-2'];
const hash=b=>createHash('sha256').update(b).digest('hex'),runnerSha256=hash(await readFile(resolve(dir,'runner.mjs')));
const runs=[];
for(const name of names){
 const r=await readResult(resolve(dir,name+'.json.gz'));assert.equal(r.completed,true);assert.equal(r.samples.length,1);
 assert.equal(r.experiment.variant,name.split('-')[0]);assert.equal(r.experiment.runnerSha256,runnerSha256);
 for(const file of ['persistence.mts','sql.mts'])assert.equal(r.experiment.controlSourceSha256[file],hash(await readFile(resolve(dir,'control',file))));
 const s=r.samples[0];assert.equal(s.status,'passed');assert.equal(s.serverVerified,true);assert.equal(s.records,10000);assert.equal(s.writes,100);
 assert.equal(s.result.verifiedRows,10100);assert.equal(s.serverVerification.rows,10100);assert.equal(s.result.accepted,100);
 assert.deepEqual(s.backendErrors,[]);assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.cleanupError,undefined);assert.equal(s.databaseCleanupError,undefined);
 assert.equal(s.result.events.length,100);assert.equal(s.serverSettlements.length,100);validateRecordReports(s);
 assert.ok(s.result.events.every(e=>e.accepted&&Number.isFinite(e.settlementUs)&&Number.isFinite(e.bVisibleUs)));
 const spans=s.serverDiagnostics.filter(d=>d.kind==='span'),groups=spans.filter(d=>d.name==='server.processBatchGroup');
 const ids=new Set(groups.map(d=>d.attributes.operationId)),byid=new Map(spans.map(d=>[d.clockDomain+'/'+d.spanId,d]));
 const queries=spans.filter(d=>d.name==='postgres.query'),mutationQueries=queries.filter(d=>ids.has(d.attributes.operationId)),breakdown={};
 for(const q of mutationQueries){let parent=q;while(parent&&!parent.name.startsWith('host.'))parent=byid.get(parent.clockDomain+'/'+parent.parentSpanId);
 assert.ok(parent,'every Mutation query has an owning host operation');const stage=breakdown[parent.name]??={count:0,ms:0,commands:{}};stage.count++;stage.ms+=q.durationUs/1000;stage.commands[q.attributes.command]=(stage.commands[q.attributes.command]??0)+1;}
 const phaseNames=new Set(['host.readTracking','host.lockStreams','host.guardRecords']);
 const phases=spans.filter(d=>ids.has(d.attributes.operationId)&&phaseNames.has(d.name)).sort((a,b)=>a.startUs-b.startUs);
 assert.deepEqual(phases.map(d=>d.name),Array.from({length:100},()=>['host.readTracking','host.lockStreams','host.guardRecords','host.readTracking']).flat());
 const after=name.startsWith('after');
 for(const[k,n]of Object.entries({'host.readTracking':200,'host.savepoint':100,'host.release':100,'host.lockStreams':100,'host.guardRecords':100,'host.targetPositions':100,'host.head':100}))assert.equal(breakdown[k].count,n);
 assert.equal(breakdown['host.applyStreamMembers'].count,200);
 assert.equal(breakdown['host.readTracking'].commands.SELECT,200);
 assert.equal(Object.keys(breakdown['host.readTracking'].commands).length,1);
 assert.equal(breakdown['host.claimStore'].count,groups.length);assert.equal(breakdown['host.saveResults'].count,groups.length);
 assert.equal(mutationQueries.length,1000+2*groups.length+breakdown['host.beginBatch'].count);
 runs.push({name,build:r.build,metrics:metrics(s),mutationGroups:groups.length,frameworkQueries:queries.length,mutationFrameworkQueries:mutationQueries.length,
 frameworkQueryMs:queries.reduce((n,d)=>n+d.durationUs/1000,0),breakdown,maxPlacementUncertaintyUs:timeline(s).clockAlignment.maxPlacementUncertaintyUs});
}
assert.ok(runs.every(r=>r.build.sourceSha256===runs[0].build.sourceSha256));
assert.ok(runs.every(r=>JSON.stringify(r.build.artifacts)===JSON.stringify(runs[0].build.artifacts)));
const summary={};for(const v of ['before','after']){const selected=runs.filter(r=>r.name.startsWith(v));summary[v]={
 metrics:Object.fromEntries(Object.keys(selected[0].metrics).map(k=>[k,statistics(selected.flatMap(r=>r.metrics[k]))])),
 counts:Object.fromEntries(['mutationGroups','frameworkQueries','mutationFrameworkQueries'].map(k=>[k,statistics(selected.map(r=>r[k]))])),
 stages:Object.fromEntries(Object.keys(selected[0].breakdown).map(k=>[k,{count:statistics(selected.map(r=>r.breakdown[k].count)),ms:statistics(selected.map(r=>r.breakdown[k].ms))}]))};}
const evidence={order:names,runnerSha256,runs,summary};await writeFile(resolve(dir,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify(summary,null,2));
