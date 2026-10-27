import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

export function statistics(values) {
  assert.ok(values.length, 'statistics needs observations');
  assert.ok(values.every(Number.isFinite), 'all observations must be finite');
  const sorted = [...values].sort((a,b)=>a-b), n=sorted.length;
  const median = n%2 ? sorted[(n-1)/2] : (sorted[n/2-1]+sorted[n/2])/2;
  return {n,median,min:sorted[0],max:sorted.at(-1),...(n>=100?{p95:sorted[Math.ceil(n*.95)-1]}:{})};
}
const ms = value => value/1000;
export function metrics(sample) {
  if (sample.scenario==='initial-sync') {
    const t=sample.result.timings, start=t.initializationStartUs;
    return {storeOpenMs:[ms(t.storeOpenUs-t.storeOpenStartUs)],firstVisibleMs:[ms(t.firstVisibleUs-start)],
      allVisibleMs:[ms(t.allVisibleUs-start)],bootstrapMs:[ms(t.bootstrapCompleteUs-start)]};
  }
  const events=sample.result.events;
  return {localCommitMs:events.map(e=>ms(e.localCommitUs-e.startUs)),
    settlementMs:events.map(e=>ms(e.settlementUs-e.startUs)),
    bVisibleMs:events.map(e=>ms(e.bVisibleUs-e.startUs)),
    allVisibleMs:[ms(sample.result.timings.allVisibleUs)],
    allSettledMs:[ms(sample.result.timings.allSettledUs)],
    entriesPerSecond:[sample.writes/(sample.result.timings.allVisibleUs/1e6)]};
}
export const caseKey = sample => `${sample.scenario}/${sample.records}/${sample.writes}`;
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
      metrics:Object.fromEntries(Object.entries(gathered).map(([name,values])=>[name,statistics(values)])),
      stages:Object.fromEntries(Object.entries(stages).map(([name,runs])=>[name,{
        summedDurationMs:statistics(runs.map(r=>r.totalMs)),maxDurationMs:statistics(runs.map(r=>r.maxMs)),
        count:statistics(runs.map(r=>r.count))}]))};
  });
}
const show = stats => stats ? `${stats.median.toFixed(3)} [${stats.min.toFixed(3)}, ${stats.max.toFixed(3)}]` : '—';
export function printSummary(result) {
  console.log('Times in ms: median [min, max]. Live per-write observations are correlated within each run.');
  console.log('| Scenario | Existing N | Writes M | Passed | Store open | First visible / local commit | All visible / settlement | Bootstrap / B visible |');
  console.log('|---|---:|---:|---:|---:|---:|---:|---:|');
  for (const group of summarize(result)) {
    const [scenario,n,m]=group.key.split('/'), x=group.metrics;
    console.log(`| ${scenario} | ${n} | ${m} | ${group.passed}/${group.runs} | ${show(x.storeOpenMs)} | ${show(x.firstVisibleMs??x.localCommitMs)} | ${show(x.allVisibleMs && scenario==='initial-sync'?x.allVisibleMs:x.settlementMs)} | ${show(x.bootstrapMs??x.bVisibleMs)} |`);
  }
}
export function compare(before,after) {
  assert.deepEqual(before.compatibility,after.compatibility,'incompatible fixture, timing boundaries, tracing, watch shape or runtime conditions');
  assert.deepEqual(before.machine,after.machine,'different machine/tool/database conditions');
  const a=summarize(before),b=summarize(after);
  assert.deepEqual(a.map(x=>x.key).sort(),b.map(x=>x.key).sort(),'different scenario matrices');
  assert.ok([...a,...b].every(x=>x.failed===0),'cannot compare results containing failed samples');
  return a.flatMap(group=>Object.entries(group.metrics).map(([metric,stats])=>{
    const next=b.find(x=>x.key===group.key).metrics[metric];
    return {case:group.key,metric,before:stats.median,after:next.median,
      changePercent:stats.median===0?null:100*(next.median/stats.median-1)};
  }));
}
if (process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  if (process.argv[2]==='compare') {
    const [before,after]=await Promise.all(process.argv.slice(3,5).map(async p=>JSON.parse(await readFile(p,'utf8'))));
    console.table(compare(before,after));
  } else printSummary(JSON.parse(await readFile(process.argv[2],'utf8')));
}
