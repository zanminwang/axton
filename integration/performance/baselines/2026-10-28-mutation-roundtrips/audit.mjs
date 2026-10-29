import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {caseKey,compare,metrics,readResult,statistics,validateRecordReports} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';
const directory=dirname(fileURLToPath(import.meta.url));
const raw=await readResult(resolve(directory,'results.json.gz'));
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
assert.equal(raw.completed,true);
assert.equal(raw.matrixCompleted,true);
assert.equal(raw.samples.length,30);
assert.equal(raw.parameters.samples,5);
assert.deepEqual(raw.parameters.sizes,[10000]);
assert.equal(raw.parameters.writes,100);
assert.deepEqual(raw.adminErrors,[]);
assert.equal(raw.compatibility.stageInstrumentation,3);
assert.equal(raw.implementation.bootstrapPrefetch,'off');
assert.equal(hash(await readFile(resolve(directory,'runner.mjs'))),raw.experiment.runnerSha256);
for(const name of ['persistence.mts','sql.mts']) assert.equal(
  hash(execFileSync('git',['show',`${raw.experiment.controlCommit}:packages/backend/postgres/src/${name}`],{cwd:directory})),
  raw.experiment.controlSource[name], 'control source is exactly the retained main commit');
const split=optimized=>({...raw,samples:raw.samples.filter(s=>s.optimized===optimized)});
const evidence={commit:raw.commit,dirty:raw.dirty,build:raw.build,experiment:raw.experiment,
  machine:raw.machine,compatibility:raw.compatibility,
  comparison:compare(split(false),split(true)),cases:[],runs:[]};
let maxPlacementUncertaintyUs=0;
for(const [index,sample] of raw.samples.entries()) {
  assert.equal(sample.status,'passed');
  assert.equal(sample.serverVerified,true);
  assert.equal(sample.result.verifiedRows,sample.records+sample.writes);
  assert.equal(sample.serverVerification.rows,sample.records+sample.writes);
  assert.equal(sample.serverVerification.writes,sample.writes);
  assert.deepEqual(sample.backendErrors,[]);
  assert.equal(sample.serverDiagnosticsDropped,0);
  assert.equal(sample.cleanupError,undefined);
  assert.equal(sample.databaseCleanupError,undefined);
  validateRecordReports(sample);
  const trace=timeline(sample);
  maxPlacementUncertaintyUs=Math.max(maxPlacementUncertaintyUs,trace.clockAlignment.maxPlacementUncertaintyUs);
  const spans=sample.serverDiagnostics.filter(r=>r.kind==='span'&&r.source==='node');
  const queries=spans.filter(r=>r.name==='postgres.query');
  const mutations=spans.filter(r=>r.name==='server.processBatchMember');
  const mutationSql=mutations.map(m=>queries.filter(q=>q.clockDomain===m.clockDomain&&q.attributes.operationId===m.attributes.operationId).length);
  if(sample.writes===1) assert.deepEqual(mutationSql,[sample.optimized?14:17]);
  evidence.runs.push({sampleIndex:index,key:caseKey(sample),optimized:sample.optimized,repetition:sample.repetition,
    sequenceIndex:sample.sequenceIndex,metrics:metrics(sample),frameworkSql:queries.length,mutationSql,
    transactions:spans.filter(r=>r.name==='server.transaction').length,
    ordinaryPulls:spans.filter(r=>r.name==='server.processDelivery05').length,
    mutationTransactions:mutations.length,recordReports:sample.result.recordReports?.length??0,
    placementUncertaintyUs:trace.clockAlignment.maxPlacementUncertaintyUs});
}
for(const key of [...new Set(raw.samples.map(caseKey))]) {
  const all=raw.samples.filter(s=>caseKey(s)===key);
  assert.deepEqual(all.map(s=>s.optimized),raw.experiment.sequence);
  assert.deepEqual(all.map(s=>s.sequenceIndex),Array.from({length:10},(_,i)=>i));
  const entry={key,pairs:[]};
  for(const optimized of [false,true]) {
    const label=optimized?'optimized':'control',samples=all.filter(s=>s.optimized===optimized);
    assert.equal(samples.length,5);
    const runs=evidence.runs.filter(r=>r.key===key&&r.optimized===optimized);
    entry[label]={metrics:Object.fromEntries(Object.keys(metrics(samples[0])).map(name=>
      [name,statistics(samples.flatMap(s=>metrics(s)[name]))])),
      frameworkSql:statistics(runs.map(r=>r.frameworkSql)),
      mutationSql:runs.map(r=>r.mutationSql),recordReports:runs.reduce((n,r)=>n+r.recordReports,0)};
    if(samples[0].writes===1) {
      const selected=[...samples].sort((a,b)=>metrics(a).bVisibleMs[0]-metrics(b).bVisibleMs[0])[2];
      await writeFile(resolve(directory,`${label}-single.trace.json`),JSON.stringify(timeline(selected))+'\n');
    }
  }
  for(let i=0;i<all.length;i+=2) {
    const pair=all.slice(i,i+2),control=pair.find(s=>!s.optimized),optimized=pair.find(s=>s.optimized);
    entry.pairs.push({order:pair.map(s=>s.optimized?'optimized':'control'),
      differenceMs:Object.fromEntries(Object.keys(metrics(control)).filter(n=>n!=='entriesPerSecond').map(name=>[
        name,statistics(metrics(optimized)[name]).median-statistics(metrics(control)[name]).median]))});
  }
  entry.pairedDifferenceMs=Object.fromEntries(Object.keys(entry.pairs[0].differenceMs).map(name=>[
    name,statistics(entry.pairs.map(p=>p.differenceMs[name]))]));
  evidence.cases.push(entry);
}
evidence.maxPlacementUncertaintyUs=maxPlacementUncertaintyUs;
await writeFile(resolve(directory,'evidence.json'),JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify({samples:raw.samples.length,maxPlacementUncertaintyUs,
  comparison:evidence.comparison.filter(r=>['bVisibleMs','allVisibleMs','settlementMs','serverTransactionMs'].includes(r.metric)),
  counts:evidence.cases.map(c=>({key:c.key,control:c.control.frameworkSql,optimized:c.optimized.frameworkSql,paired:c.pairedDifferenceMs}))},null,2));
