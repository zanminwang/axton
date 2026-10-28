import {readResult,statistics,serverSettlements,validateRecordReports,summarize,compare} from '../../report.mjs';
import {writeFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
const r=await readResult(fileURLToPath(new URL('./results.json.gz',import.meta.url)));
assert.equal(r.commit,'02bdaf0bb9ce8c0ceb290414444f5ad1557a6ef7');assert.equal(r.dirty,false);assert.equal(r.completed,true);assert.equal(r.matrixCompleted,true);assert.equal(r.samples.length,45);
let accepted=0,correlated=0,reports=0;
for(const s of r.samples){assert.equal(s.status,'passed');assert.equal(s.serverVerified,true);validateRecordReports(s);accepted+=s.result.accepted??0;correlated+=serverSettlements(s).length;reports+=s.result.recordReports?.length??0;assert.equal(s.backendErrors.length,0);assert.equal(s.serverDiagnosticsDropped,0);assert.equal(s.result.dropped,0);assert.equal((s.progress??[]).filter(p=>p.phase==='connection.error').length,0);assert.ok(!s.cleanupError&&!s.databaseCleanupError);}
assert.equal(accepted,1515);assert.equal(correlated,1515);assert.equal(r.adminErrors.length,0);
const medStats=arr=>statistics(arr);
const collect=(runs, get)=>{const names=new Set(runs.flatMap(s=>Object.keys(get(s))));return Object.fromEntries([...names].sort().map(name=>[name,{count:medStats(runs.map(s=>get(s)[name]?.count??0)),sumMs:medStats(runs.map(s=>get(s)[name]?.sumMs??0))}]));};
const bucket=records=>{const out={};for(const x of records){if(x.kind!=='span')continue;const k=`${x.source}/${x.name}`,b=out[k]??={count:0,sumMs:0};b.count++;b.sumMs+=x.durationUs/1000;}return out;};
const upload=s=>{const opIds=new Set(s.serverDiagnostics.filter(x=>x.source==='node'&&x.name==='host.handleAction'&&x.attributes.mutation==='CreateEntry').map(x=>x.attributes.operationId));return s.serverDiagnostics.filter(x=>opIds.has(x.attributes.operationId));};
const sql=s=>{const records=upload(s),parents=new Map(records.filter(x=>x.source==='node').map(x=>[`${x.clockDomain}/${x.spanId}`,x]));const out={};for(const q of records.filter(x=>x.source==='node'&&x.name==='postgres.query')){const p=parents.get(`${q.clockDomain}/${q.parentSpanId}`),k=p?.name??'unknown',b=out[k]??={count:0,sumMs:0};b.count++;b.sumMs+=q.durationUs/1000;}return out;};
const invalidations=s=>{const out={};for(const x of s.result.diagnostics.filter(x=>x.client==='B'&&x.name==='watch.invalidate')){const a=x.attributes,delta=a.toGeneration-a.fromGeneration;const suffix=delta===1?(/"Entry"/.test(a.lastCommitTables)?'Entry-written':'metadata-only'):'multi-commit-unknown';const k=`${a.cause}/${suffix}/${a.lastCommitTables}`,b=out[k]??={count:0,sumMs:0};b.count++;}return out;};
const groups=summarize(r).map(g=>{const ss=r.samples.filter(s=>`${s.scenario}/${s.records}/${s.writes}`===g.key);assert.equal(ss.length,5);return {...g,uploadStages:collect(ss,s=>bucket(upload(s))),uploadSQLByHost:collect(ss,sql),bStages:collect(ss,s=>bucket(s.result.diagnostics.filter(x=>x.client==='B'))),bInvalidations:collect(ss,invalidations),bComparison:collect(ss,s=>{const out={};for(const q of s.result.diagnostics.filter(x=>x.client==='B'&&x.name==='watch.compare')){const k=String(q.attributes.changed),b=out[k]??={count:0,sumMs:0};b.count++;b.sumMs+=q.durationUs/1000;}return out;})};});
const e={sourceCommit:r.commit,dirty:r.dirty,machine:r.machine,parameters:r.parameters,compatibility:r.compatibility,totals:{attempted:45,passed:45,serverVerified:45,accepted,correlated,recordReports:reports,backendErrors:0,connectionErrors:0,traceLoss:0,cleanupErrors:0,adminErrors:0},groups};
const previous=await readResult(fileURLToPath(new URL('../2026-10-27-startup-layout/results.json.gz',import.meta.url)));
try { compare(previous,r); throw new Error('previous instrumentation unexpectedly compatible'); }
catch(error) { assert.match(error.message,/incompatible fixture/); e.previousComparison={admitted:false,reason:error.message.split('\n')[0],previousCommit:previous.commit}; }
e.provenance={workflowRun:40630070383,job:122624147912,artifactId:12790928621,artifactArchiveSha256:'dc2c970e3f013e02e304354db51d6a5f8123057292d47ed81ae21d0cc1246148'};
await writeFile(new URL('./evidence.json',import.meta.url),JSON.stringify(e,null,2)+'\n');
console.log(JSON.stringify(e.totals));
