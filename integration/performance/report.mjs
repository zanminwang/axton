import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {gunzipSync} from 'node:zlib';

export async function readResult(path) {
  const bytes=await readFile(path);
  return JSON.parse((path.endsWith('.gz')?gunzipSync(bytes):bytes).toString('utf8'));
}

export function statistics(values) {
  assert.ok(values.length, 'statistics needs observations');
  assert.ok(values.every(Number.isFinite), 'all observations must be finite');
  const sorted = [...values].sort((a,b)=>a-b), n=sorted.length;
  const median = n%2 ? sorted[(n-1)/2] : (sorted[n/2-1]+sorted[n/2])/2;
  return {n,median,min:sorted[0],max:sorted.at(-1),...(n>=100?{p95:sorted[Math.ceil(n*.95)-1]}:{})};
}
const ms = value => value/1000;
// A named CreateEntry has one host action, and each fresh benchmark Store's
// mutation ordinal starts at 1. The transaction span ends after adapter COMMIT.
// Keep its source clock; never subtract it from the clients' Stopwatch.
export function serverSettlements(sample) {
  if(sample.scenario!=='live-sync')return [];
  const records=sample.serverDiagnostics??[], transactions=new Map();
  for(const r of records)if(r.source==='node'&&r.name==='server.transaction'&&r.attributes.outcome==='success')
    transactions.set(`${r.clockDomain}/${r.attributes.operationId}`,r);
  const observed=new Map();
  for(const r of records) {
    if(r.source!=='node'||r.name!=='host.handleAction'||r.attributes.mutation!=='CreateEntry'||r.attributes.outcome!=='success')continue;
    // Server callId = Store identity : batch ID : durable Mutation ID.
    const ordinal=Number(/:\d+:(\d+)$/.exec(r.attributes.callId??'')?.[1]);
    const event=sample.result?.events?.[ordinal-1];
    const tx=transactions.get(`${r.clockDomain}/${r.attributes.operationId}`);
    if(!event||!tx)continue;
    const committedUs=tx.startUs+tx.durationUs, previous=observed.get(event.id);
    if(!previous||committedUs<previous.committedUs)observed.set(event.id,{
      id:event.id,ordinal,callId:r.attributes.callId,clockDomain:tx.clockDomain,
      transactionSpanId:tx.spanId,transactionStartUs:tx.startUs,committedUs,transactionDurationUs:tx.durationUs,
    });
  }
  return [...observed.values()].sort((a,b)=>a.ordinal-b.ordinal);
}
export function metrics(sample) {
  if (sample.scenario==='initial-sync') {
    const t=sample.result.timings, start=t.initializationStartUs;
    return {storeOpenMs:[ms(t.storeOpenUs-t.storeOpenStartUs)],firstVisibleMs:[ms(t.firstVisibleUs-start)],
      allVisibleMs:[ms(t.allVisibleUs-start)],bootstrapMs:[ms(t.bootstrapCompleteUs-start)]};
  }
  const events=sample.result.events;
  const server=serverSettlements(sample);
  return {localCommitMs:events.map(e=>ms(e.localCommitUs-e.startUs)),
    ...(server.length?{serverTransactionMs:server.map(e=>ms(e.transactionDurationUs))}:{}),
    settlementMs:events.map(e=>ms(e.settlementUs-e.startUs)),
    bVisibleMs:events.map(e=>ms(e.bVisibleUs-e.startUs)),
    allVisibleMs:[ms(sample.result.timings.allVisibleUs)],
    allSettledMs:[ms(sample.result.timings.allSettledUs)],
    entriesPerSecond:[sample.writes/(sample.result.timings.allVisibleUs/1e6)]};
}
export const caseKey = sample => `${sample.scenario}/${sample.records}/${sample.writes}`;
export function validateRecordReports(sample) {
  for (const report of sample.result.recordReports??[]) {
    const index=sample.result.events.findIndex(event=>event.id===report.identity?.id&&event.accepted===true);
    const end=Math.max(sample.result.timings.allSettledUs,sample.result.timings.allVisibleUs);
    assert.ok(sample.scenario==='live-sync'&&report.client==='A'&&report.kind==='diverged'
      &&report.model==='Entry'&&report.duringMeasurement===true&&index>=0&&report.ordinal===index+1
      &&Number.isFinite(report.atUs)&&report.atUs>=sample.result.events[index].startUs&&report.atUs<=end,
      'unexpected record report: only transient divergence of a verified accepted own create is admissible');
  }
}
export function summarize(result) {
  const groups=new Map();
  for (const sample of result.samples) {
    const key=caseKey(sample); if (!groups.has(key)) groups.set(key,[]);
    groups.get(key).push(sample);
  }
  return [...groups].map(([key,samples])=>{
    const passed=samples.filter(s=>s.status==='passed'), gathered={};
    for (const sample of passed) for (const [name,values] of Object.entries(metrics(sample)))
      (gathered[name]??=[]).push(...values);
    const stages={};
    for (const sample of passed) {
      const perRun={};
      for (const record of [...sample.result.diagnostics,...sample.serverDiagnostics]) {
        if (record.kind!=='span') continue;
        const name=`${record.source}/${record.name}`;
        const stage=perRun[name]??={count:0,totalMs:0,maxMs:0};
        stage.count++; stage.totalMs+=ms(record.durationUs); stage.maxMs=Math.max(stage.maxMs,ms(record.durationUs));
      }
      for (const [name,value] of Object.entries(perRun)) (stages[name]??=[]).push(value);
    }
    return {key,runs:samples.length,passed:passed.length,failed:samples.length-passed.length,
      recordReports:passed.reduce((n,s)=>n+(s.result.recordReports?.length??0),0),
      failedRecordReports:samples.filter(s=>s.status!=='passed').reduce((n,s)=>n+(s.result?.recordReports?.length??0),0),
      metrics:Object.fromEntries(Object.entries(gathered).map(([name,values])=>[name,statistics(values)])),
      stages:Object.fromEntries(Object.entries(stages).map(([name,runs])=>[name,{
        summedDurationMs:statistics(runs.map(r=>r.totalMs)),maxDurationMs:statistics(runs.map(r=>r.maxMs)),
        count:statistics(runs.map(r=>r.count))}]))};
  });
}
const show = stats => stats ? `${stats.median.toFixed(3)} [${stats.min.toFixed(3)}, ${stats.max.toFixed(3)}]` : '—';
export function printSummary(result) {
  if(!result.completed)console.log(result.matrixCompleted
    ?'FAILED RUN: full matrix attempted; failures are retained. Not a clean baseline.'
    :'INCOMPLETE RUN: partial samples are retained, not a complete baseline.');
  console.log('Times in ms: median [min, max]. Live per-write observations are correlated within each run.');
  const traces=result.samples.filter(s=>s.timeline),aligned=traces.filter(s=>s.timeline.status==='aligned');
  if(traces.length) {
    const uncertainty=aligned.length?Math.max(...aligned.map(s=>s.timeline.clockAlignment.maxPlacementUncertaintyUs)):null;
    console.log(`Common-axis traces: ${aligned.length}/${traces.length} aligned${uncertainty===null?'':`; maximum placement uncertainty ±${uncertainty.toFixed(1)} µs`}. Paths and calibration bounds are retained per sample.`);
    for(const s of traces.filter(s=>s.timeline.status!=='aligned'))console.log(`Trace unavailable ${caseKey(s)} r${s.repetition}: ${s.timeline.reason}`);
  }
  const groups=summarize(result);
  console.log('\nInitial sync:');
  console.log('| Existing N | Passed | Store open | First visible | All visible | Bootstrap |');
  console.log('|---:|---:|---:|---:|---:|---:|');
  for (const group of groups.filter(g=>g.key.startsWith('initial-sync/'))) {
    const [,n]=group.key.split('/'),x=group.metrics;
    console.log(`| ${n} | ${group.passed}/${group.runs} | ${show(x.storeOpenMs)} | ${show(x.firstVisibleMs)} | ${show(x.allVisibleMs)} | ${show(x.bootstrapMs)} |`);
  }
  console.log('\nLive sync:');
  console.log('| Existing N | Writes M | Passed | Reports passed/failed | Local commit | Server transaction | A settlement | B visible | All B visible |');
  console.log('|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|');
  for (const group of groups.filter(g=>g.key.startsWith('live-sync/'))) {
    const [,n,m]=group.key.split('/'),x=group.metrics;
    console.log(`| ${n} | ${m} | ${group.passed}/${group.runs} | ${group.recordReports}/${group.failedRecordReports} | ${show(x.localCommitMs)} | ${show(x.serverTransactionMs)} | ${show(x.settlementMs)} | ${show(x.bVisibleMs)} | ${show(x.allVisibleMs)} |`);
  }
  console.log('\nSelected stages: median summed span ms per run; nested/overlapping spans are not additive. Live includes both clients.');
  console.log('| Case | Server plan | Mutation execute | Local apply | Watch query | Bridge drain/decode |');
  console.log('|---|---:|---:|---:|---:|---:|');
  for (const group of groups) {
    const values=['rust/delivery.plan','rust/mutation.execute','rust/delivery.apply','rust/watch.query','dart/bridge.drain']
      .map(name=>group.stages[name]?.summedDurationMs.median.toFixed(3)??'—');
    console.log(`| ${group.key} | ${values.join(' | ')} |`);
  }
  console.log('\nDetailed measured stages: summed ms per run; overlapping, not additive.');
  console.log('| Case | SQL calls | Pool acquire | COMMIT | Rust callback wait | Handler |');
  console.log('|---|---:|---:|---:|---:|---:|');
  for (const g of groups) {
    const v=['node/postgres.query','node/postgres.acquire','node/postgres.commit','rust/host.await','node/handler.invoke'].map(k=>g.stages[k]?.summedDurationMs.median.toFixed(3)??'—');
    console.log(`| ${g.key} | ${v.join(' | ')} |`);
  }
  console.log('\n| Case | Watch read | Convert | Compare | Snapshot | Dart JSON decode |');
  console.log('|---|---:|---:|---:|---:|---:|');
  for (const g of groups) {
    const v=['rust/watch.read','rust/watch.convert','rust/watch.compare','rust/watch.snapshot','dart/bridge.decode'].map(k=>g.stages[k]?.summedDurationMs.median.toFixed(3)??'—');
    console.log(`| ${g.key} | ${v.join(' | ')} |`);
  }
}
export function compare(before,after,{scenario}={}) {
  assert.ok(scenario===undefined||['initial-sync','live-sync'].includes(scenario),'unknown comparison scenario');
  assert.ok([before,after].every(r=>r.completed||(scenario&&r.matrixCompleted)),
    'cannot compare incomplete runs; select a fully passing scenario only after the full matrix was attempted');
  assert.deepEqual(before.compatibility,after.compatibility,'incompatible fixture, timing boundaries, tracing, watch shape or runtime conditions');
  assert.deepEqual(before.machine,after.machine,'different machine/tool/database conditions');
  const selected=result=>summarize(result).filter(g=>!scenario||g.key.startsWith(scenario+'/'));
  const a=selected(before),b=selected(after);
  assert.ok(a.length&&b.length,'no observations for comparison');
  assert.deepEqual(a.map(x=>x.key).sort(),b.map(x=>x.key).sort(),'different scenario matrices');
  assert.ok([...a,...b].every(x=>x.failed===0),'cannot compare results containing failed samples');
  return a.flatMap(group=>Object.entries(group.metrics).map(([metric,stats])=>{
    const next=b.find(x=>x.key===group.key).metrics[metric];
    return {case:group.key,metric,before:stats.median,after:next.median,
      beforeRecordReports:group.recordReports,afterRecordReports:b.find(x=>x.key===group.key).recordReports,
      changePercent:stats.median===0?null:100*(next.median/stats.median-1)};
  }));
}
if (process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  if (process.argv[2]==='compare') {
    const [before,after]=await Promise.all(process.argv.slice(3,5).map(readResult));
    const extra=process.argv.slice(5);
    assert.ok(extra.length===0||(extra.length===2&&extra[0]==='--scenario'),'usage: compare before.json after.json [--scenario initial-sync|live-sync]');
    console.table(compare(before,after,{scenario:extra[1]}));
  } else printSummary(await readResult(process.argv[2]));
}
