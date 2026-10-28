import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {calibration,combineCalibrations,clockRecord} from './support/clock.mjs';

function endpointPair(domain, reference, endpoints, method) {
  assert.ok(endpoints?.start && endpoints?.end, `missing before/after clock probes for ${domain}`);
  return combineCalibrations(calibration(domain, reference, endpoints.start, method),
    calibration(domain, reference, endpoints.end, method));
}

export function clockAlignment(sample) {
  const runner=sample.clockCorrelation, child=sample.result?.clockCorrelation;
  assert.ok(runner&&child, 'raw sample has no clock calibration; old traces cannot be placed on an exact common axis');
  const root=runner.runnerClockDomain;
  const remote=runner.dartRunnerProbes;
  assert.ok(remote?.start?.clockDomain===child.referenceClockDomain&&remote?.end?.clockDomain===child.referenceClockDomain,
    'Dart reference clock changed');
  const links=[endpointPair(child.referenceClockDomain,root,
    {start:remote.start.probes,end:remote.end.probes},'stdio-round-trip-bounds')];
  links.push(endpointPair(child.dartClockDomain,child.referenceClockDomain,child.dartProbes,'shared-isolate-Stopwatch-bracket'));
  if([...(sample.result.diagnostics??[]),...(sample.serverDiagnostics??[])].some(r=>r.source==='rust')) {
    links.push(endpointPair(runner.nodeNativeClockDomain,root,runner.nodeNativeProbes,'synchronous-NAPI-bracket'));
    links.push(endpointPair(child.nativeClockDomain,child.referenceClockDomain,child.nativeProbes,'synchronous-FFI-bracket'));
  }
  const records=[...links.map(edge=>clockRecord(edge)),...(child.clockRecords??[]),
    ...(sample.result.clockRecords??[]),...(sample.serverDiagnostics??[]).filter(r=>r.kind==='clock')];
  const edges=new Map();
  for(const r of records) {
    const a=r.attributes;
    assert.ok(typeof a.referenceClockDomain==='string'&&Number.isFinite(a.offsetLowerUs)&&Number.isFinite(a.offsetUpperUs)
      &&a.offsetLowerUs<=a.offsetUpperUs,'valid clock-origin bounds');
    const edge={...a,clockDomain:r.clockDomain}, previous=edges.get(r.clockDomain);
    if(previous) {
      assert.equal(previous.referenceClockDomain,edge.referenceClockDomain,'clock has conflicting reference domains');
      edge.offsetLowerUs=Math.max(previous.offsetLowerUs,edge.offsetLowerUs);
      edge.offsetUpperUs=Math.min(previous.offsetUpperUs,edge.offsetUpperUs);
      assert.ok(edge.offsetLowerUs<=edge.offsetUpperUs,'clock origins disagree');
    }
    edges.set(r.clockDomain,edge);
  }
  const offsets=new Map([[root,{lowerUs:0,upperUs:0}]]), visiting=new Set();
  const offset=domain=>{
    if(offsets.has(domain))return offsets.get(domain);
    assert.ok(!visiting.has(domain),'clock-correlation cycle');visiting.add(domain);
    const edge=edges.get(domain);assert.ok(edge,`uncalibrated clock domain ${domain}`);
    const parent=offset(edge.referenceClockDomain);
    const value={lowerUs:parent.lowerUs+edge.offsetLowerUs,upperUs:parent.upperUs+edge.offsetUpperUs};
    visiting.delete(domain);offsets.set(domain,value);return value;
  };
  const origin=offset(child.measurementClockDomain), zeroUs=(origin.lowerUs+origin.upperUs)/2;
  const map=(domain,rawUs)=>{
    const o=offset(domain);
    return {atUs:rawUs+(o.lowerUs+o.upperUs)/2-zeroUs,
      uncertaintyUs:(o.upperUs-o.lowerUs+origin.upperUs-origin.lowerUs)/2+1,
      offsetLowerUs:o.lowerUs,offsetUpperUs:o.upperUs};
  };
  return {map,measurementDomain:child.measurementClockDomain,
    metadata:{referenceClockDomain:root,originClockDomain:child.measurementClockDomain,
      zeroReferenceUs:zeroUs,edges:[...edges.values()],
      model:'constant monotonic-clock offsets; before/after probe intervals must intersect',
      caveat:'Placement uses interval midpoints. Retained bounds include transport/scheduling uncertainty; no exact one-way network latency or CPU-time inference.'}};
}

function milestones(sample, domain) {
  const span=(name,lane,start,end,attributes={})=>({kind:'span',source:'benchmark',clockDomain:domain,
    name,lane,startUs:start,durationUs:end-start,attributes});
  const event=(name,lane,startUs,attributes={})=>({kind:'event',source:'benchmark',clockDomain:domain,name,lane,startUs,attributes});
  const t=sample.result.timings, records=[];
  if(sample.scenario==='initial-sync') {
    records.push(span('Initial synchronization (elapsed)','Benchmark',t.initializationStartUs,
      Math.max(t.bootstrapCompleteUs,t.allVisibleUs)),
    span('Store open (elapsed)','A milestones',t.storeOpenStartUs,t.storeOpenUs));
    for(const [key,value] of Object.entries(t))if(Number.isFinite(value))records.push(event(key,'A milestones',value));
    for(const snapshot of sample.result.snapshots??[])records.push(event('watch visible','A milestones',snapshot.atUs,{rows:snapshot.rows}));
  } else {
    records.push(span('Live synchronization (elapsed)','Benchmark',0,Math.max(t.allSettledUs,t.allVisibleUs)));
    for(const e of sample.result.events??[]) {
      records.push(span('Local mutation submission (elapsed)','A milestones',e.startUs,e.localCommitUs,{identity:e.id}),
        event('A write starts','A milestones',e.startUs,{identity:e.id}),
        event('A locally committed','A milestones',e.localCommitUs,{identity:e.id}),
        event('A settled','A milestones',e.settlementUs,{identity:e.id}),
        event('B watch visible','B milestones',e.bVisibleUs,{identity:e.id}));
    }
  }
  return records;
}

// Allocate a separate track for concurrent siblings rather than letting Chrome
// infer a false parent from overlapping asynchronous spans. Real parent IDs are
// retained even when a concurrent branch needs a different visual track.
function placeTrees(spans, allocateTrack) {
  const start=r=>r.alignedStartUs;
  const byId=new Map(spans.filter(r=>r.spanId).map(r=>[`${r.clockDomain}/${r.spanId}`,r]));
  assert.equal(byId.size,spans.filter(r=>r.spanId).length,'duplicate span identity');
  const children=new Map(), roots=[];
  for(const r of spans) {
    const parent=r.parentSpanId?byId.get(`${r.clockDomain}/${r.parentSpanId}`):undefined;
    if(parent) {
      assert.ok(r.startUs>=parent.startUs-2&&r.startUs+r.durationUs<=parent.startUs+parent.durationUs+2,
        `child span outside its parent: ${r.name}`);
      const siblings=children.get(parent)??[];siblings.push(r);children.set(parent,siblings);
    } else {
      assert.ok(!r.parentSpanId,`missing parent ${r.clockDomain}/${r.parentSpanId}`);
      roots.push(r);
    }
  }
  const placed=[];
  const visit=(siblings,preferred)=>{
    const tracks=[];
    for(const r of siblings.sort((a,b)=>start(a)-start(b)||b.durationUs-a.durationUs)) {
      let slot=tracks.find(s=>s.end<=start(r));
      if(!slot) {slot={track:tracks.length===0?preferred:allocateTrack(),end:0};tracks.push(slot);}
      slot.end=start(r)+r.durationUs;
      placed.push({record:r,track:slot.track});
      visit(children.get(r)??[],slot.track);
    }
  };
  visit(roots,allocateTrack());
  assert.equal(placed.length,spans.length,'span-parent cycle or duplicate span identity');
  return placed;
}

export function timeline(sample) {
  assert.ok(sample.result?.timings,'sample has no completed milestone interval');
  const alignment=clockAlignment(sample), metadata=alignment.metadata;
  const raw=[...(sample.result.diagnostics??[]),...(sample.serverDiagnostics??[])];
  assert.ok(!raw.some(r=>r.kind==='loss')&&!(sample.result.dropped>0)&&!(sample.serverDiagnosticsDropped>0),
    'trace loss prevents a complete timeline');
  const records=raw.filter(r=>r.kind==='span'||r.kind==='event');
  const observed=records.map(r=>({...r,lane:r.client?`${r.client} ${r.source}`:`Server ${r.source}`}));
  const benchmark=milestones(sample,alignment.measurementDomain);
  const setup=sample.result.clockCorrelation.setupSpans??[];
  const traceEvents=[{ph:'M',name:'process_name',pid:1,tid:0,args:{name:`${sample.scenario} N=${sample.records} M=${sample.writes} r=${sample.repetition}`}}];
  const lanes=new Map(), groups=new Map();let nextTrack=0,maxUncertaintyUs=0;
  const allocate=(name)=>{const tid=++nextTrack;traceEvents.push({ph:'M',name:'thread_name',pid:1,tid,args:{name}});return tid;};
  const emit=(r,tid)=>{
    assert.ok(Number.isFinite(r.startUs)&&(!('durationUs'in r)||(Number.isFinite(r.durationUs)&&r.durationUs>=0)),`valid span time: ${r.name}`);
    const mapped=alignment.map(r.clockDomain,r.startUs);maxUncertaintyUs=Math.max(maxUncertaintyUs,mapped.uncertaintyUs);
    traceEvents.push({name:r.name,cat:r.source,ph:r.kind==='span'?'X':'i',...(r.kind==='span'?{dur:r.durationUs}:{s:'t'}),
      pid:1,tid,ts:mapped.atUs,args:{...r.attributes,clockDomain:r.clockDomain,rawStartUs:r.startUs,
        ...(r.durationUs!==undefined?{rawDurationUs:r.durationUs}:{}),spanId:r.spanId,parentSpanId:r.parentSpanId,
        alignmentUncertaintyUs:mapped.uncertaintyUs,referenceOffsetLowerUs:mapped.offsetLowerUs,referenceOffsetUpperUs:mapped.offsetUpperUs}});
  };
  for(const r of [...observed,...benchmark,...setup]) {
    r.alignedStartUs=alignment.map(r.clockDomain,r.startUs).atUs;
    const group=groups.get(r.lane)??[];group.push(r);groups.set(r.lane,group);
  }
  for(const [name,group]of groups) {
    let branch=0;
    const allocateBranch=()=>allocate(branch++===0?name:`${name} · concurrent ${branch}`);
    const placements=placeTrees(group.filter(r=>r.kind==='span'),allocateBranch);
    const primary=placements[0]?.track??allocateBranch();lanes.set(name,primary);
    for(const {record,track}of placements)emit(record,track);
    for(const r of group.filter(r=>r.kind==='event'))emit(r,primary);
  }
  const measured=benchmark.find(r=>r.lane==='Benchmark'), end=measured.startUs+measured.durationUs;
  const intervals=observed.filter(r=>r.kind==='span').map(r=>{
    const start=alignment.map(r.clockDomain,r.startUs).atUs;
    return [Math.max(0,start),Math.min(end,start+r.durationUs)];
  }).filter(([a,b])=>b>a).sort((a,b)=>a[0]-b[0]);
  const unattributed=[];let covered=0;
  for(const [start,finish]of intervals) {if(start>covered)unattributed.push([covered,start]);covered=Math.max(covered,finish);}
  if(covered<end)unattributed.push([covered,end]);
  const gaps=allocate('Coverage gaps · cause unknown');
  for(const [start,finish]of unattributed)emit({kind:'span',name:'No recorded source span (unattributed)',source:'coverage',
    clockDomain:alignment.measurementDomain,startUs:start,durationUs:finish-start,attributes:{
      meaning:'No recorded source span covers this interval; it is not a measured network, queue or idle phase.'}},gaps);
  return {schemaVersion:1,displayTimeUnit:'ms',traceEvents,
    clockAlignment:{...metadata,maxPlacementUncertaintyUs:maxUncertaintyUs},
    unattributedIntervals:unattributed.length,
    notes:['X durations are elapsed source spans, not CPU samples.','Nested/overlapping spans are not added.',
      'Concurrent siblings use separate tracks; original spanId/parentSpanId are retained.',
      'The measured scenario starts at zero. Any late-flushed setup span retains its actual position outside that interval.',
      'Clock probes remain metadata; no old raw archive is silently aligned.']};
}

if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const {readResult}=await import('./report.mjs');
  const [path,index='0',output='artifacts/performance/sample.trace.json']=process.argv.slice(2);
  assert.ok(path,'usage: timeline.mjs results.json [sample-index] [output.trace.json]');
  const result=await readResult(path), trace=timeline(result.samples[Number(index)]);
  await writeFile(output,JSON.stringify(trace)+'\n');
  console.log(`${output}: common time axis, maximum placement uncertainty ±${trace.clockAlignment.maxPlacementUncertaintyUs.toFixed(1)} µs`);
}
