import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {dirname,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {caseKey,compare,metrics,readResult,statistics,validateRecordReports} from '../../report.mjs';
import {timeline} from '../../timeline.mjs';

const directory=dirname(fileURLToPath(import.meta.url));
const data={off:await readResult(resolve(directory,'off.json.gz')),
  on:await readResult(resolve(directory,'on.json.gz'))};
const comparison=compare(data.off,data.on);
assert.equal(data.off.commit,data.on.commit,'same implementation commit');
assert.deepEqual(data.off.build,data.on.build,'identical release binaries and source');
assert.equal(data.off.implementation.bootstrapPrefetch,'off');
assert.equal(data.on.implementation.bootstrapPrefetch,'one-page opt-in');
const evidence={comparison,commit:data.off.commit,build:data.off.build,
  compatibility:data.off.compatibility,machine:data.off.machine,cases:[]};
const interval=e=>({startMs:e.ts/1000,endMs:(e.ts+e.dur)/1000,durationMs:e.dur/1000});
function initial(sample,trace) {
  const spans=trace.traceEvents.filter(e=>e.ph==='X');
  const pulls=spans.filter(e=>e.name==='http.request'&&e.args.route==='pull').sort((a,b)=>a.ts-b.ts);
  const apply=spans.filter(e=>e.name==='delivery.apply').sort((a,b)=>a.ts-b.ts);
  assert.equal(pulls.length,20,'unchanged 500-row pages');
  assert.equal(apply.length,20,'twenty complete-unit commits');
  assert.deepEqual(sample.result.snapshots.map(s=>s.rows),Array.from({length:21},(_,i)=>i*500));
  const nextPullRelativeToPreviousApplyEndMs=pulls.slice(1).map((p,i)=>(p.ts-apply[i].ts-apply[i].dur)/1000);
  const overlapMs=pulls.flatMap(p=>apply.map(a=>Math.max(0,Math.min(p.ts+p.dur,a.ts+a.dur)-Math.max(p.ts,a.ts))/1000))
    .reduce((a,b)=>a+b,0);
  const transactions=spans.filter(e=>e.cat==='node'&&e.name==='server.transaction').map(e=>({...interval(e),
    operationId:e.args.operationId,operations:spans.filter(s=>s.args.operationId===e.args.operationId&&s.name.startsWith('server.process')).map(s=>s.name)}));
  const names=['delivery.plan','delivery.apply','watch.query','watch.read','watch.convert','bridge.drain','bridge.decode','watch.enqueue'];
  return {metrics:metrics(sample),snapshots:sample.result.snapshots,
    pulls:pulls.map(p=>({...interval(p),responseBytes:p.args.responseBytes})),apply:apply.map(interval),transactions,
    watch:spans.filter(e=>e.name==='watch.query').map(interval),
    nextPullRelativeToPreviousApplyEndMs,overlapMs,
    pageTiming:{pullDurationMs:statistics(pulls.map(e=>e.dur/1000)),
      pullStartCycleMs:statistics(pulls.slice(1).map((e,i)=>(e.ts-pulls[i].ts)/1000)),
      betweenRequestsMs:statistics(pulls.slice(1).map((e,i)=>(e.ts-pulls[i].ts-pulls[i].dur)/1000)),
      applyDurationMs:statistics(apply.map(e=>e.dur/1000)),
      responseToApplyMs:statistics(apply.map((e,i)=>(e.ts-pulls[i].ts-pulls[i].dur)/1000))},
    cumulativeStages:Object.fromEntries(names.map(name=>{
      const s=spans.filter(e=>e.name===name);return [name,{count:s.length,totalMs:s.reduce((n,e)=>n+e.dur/1000,0)}];
    })),maxPlacementUncertaintyUs:trace.clockAlignment.maxPlacementUncertaintyUs};
}
for(const [label,result]of Object.entries(data)) {
  assert.equal(result.completed,true);
  assert.equal(result.matrixCompleted,true);
  assert.equal(result.dirty,false,'measured clean implementation');
  assert.equal(result.compatibility.stageInstrumentation,3);
  assert.equal(result.parameters.samples,5);
  assert.deepEqual(result.parameters.sizes,[10000]);
  assert.equal(result.parameters.writes,100);
  assert.equal(result.samples.length,15);
  let maxPlacementUncertaintyUs=0;
  const initialRuns=[];
  for(const sample of result.samples) {
    assert.equal(sample.status,'passed');
    assert.equal(sample.serverVerified,true);
    assert.equal(sample.backendErrors.length,0);
    validateRecordReports(sample);
    const trace=timeline(sample);
    maxPlacementUncertaintyUs=Math.max(maxPlacementUncertaintyUs,trace.clockAlignment.maxPlacementUncertaintyUs);
    if(sample.scenario==='initial-sync'){
      const run=initial(sample,trace);
      assert.equal(run.pulls.reduce((n,p)=>n+p.responseBytes,0),6214756,'same complete payload volume');
      assert.ok(run.nextPullRelativeToPreviousApplyEndMs.every(value=>label==='off'?value>0:value<0),
        'serial successors follow apply; enabled successors start before predecessor apply ends');
      initialRuns.push(run);
    }
  }
  const selected=result.samples.filter(s=>s.scenario==='initial-sync')
    .sort((a,b)=>metrics(a).allVisibleMs[0]-metrics(b).allVisibleMs[0])[2];
  const trace=timeline(selected);
  await writeFile(`${directory}/${label}-selected.trace.json`,JSON.stringify(trace)+'\n');
  evidence[label]={sampleCount:result.samples.length,maxPlacementUncertaintyUs,
    implementation:result.implementation,
    recordReports:result.samples.reduce((n,s)=>n+(s.result.recordReports?.length??0),0),
    initialRuns:initialRuns.map(r=>({metrics:r.metrics,overlapMs:r.overlapMs,
      nextPullRelativeToPreviousApplyEndMs:statistics(r.nextPullRelativeToPreviousApplyEndMs),
      pullBytes:r.pulls.reduce((n,p)=>n+p.responseBytes,0)})),
    selected:{sampleIndex:result.samples.indexOf(selected),repetition:selected.repetition,...initial(selected,trace)}};
}
for(const key of [...new Set(data.off.samples.map(caseKey))]) {
  const entry={key};
  for(const [label,result]of Object.entries(data)) {
    const samples=result.samples.filter(s=>caseKey(s)===key);
    entry[label]=Object.fromEntries(Object.keys(metrics(samples[0])).map(name=>[name,statistics(samples.flatMap(s=>metrics(s)[name]))]));
  }
  evidence.cases.push(entry);
}
await writeFile(`${directory}/evidence.json`,JSON.stringify(evidence,null,2)+'\n');
console.log(JSON.stringify({commit:evidence.commit,comparison,
  off:{initialRuns:evidence.off.initialRuns,maxPlacementUncertaintyUs:evidence.off.maxPlacementUncertaintyUs},
  on:{initialRuns:evidence.on.initialRuns,maxPlacementUncertaintyUs:evidence.on.maxPlacementUncertaintyUs}},null,2));
