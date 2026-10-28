import test from 'node:test';
import assert from 'node:assert/strict';
import {calibration,combineCalibrations,probeBounds} from './support/clock.mjs';
import {timeline} from './timeline.mjs';

const probe=(offset,time=10000)=>({localSendUs:offset+time-3,remoteReceiveUs:time,
  remoteSendUs:time,localReceiveUs:offset+time+3});
const pair=offset=>({start:[probe(offset)],end:[probe(offset,100000)]});
const clock=(domain,reference,offset)=>({kind:'clock',name:'clock.origin',clockDomain:domain,
  attributes:{referenceClockDomain:reference,offsetLowerUs:offset,offsetUpperUs:offset}});
const span=(name,domain,startUs,durationUs,spanId,parentSpanId,source='rust',client)=>({
  kind:'span',name,clockDomain:domain,startUs,durationUs,spanId,parentSpanId,source,client,attributes:{}});

function sample() {
  return {scenario:'initial-sync',records:100,writes:0,repetition:1,
    clockCorrelation:{runnerClockDomain:'node:runner',nodeNativeClockDomain:'native:runner',nodeNativeProbes:pair(1000200),
      dartRunnerProbes:{start:{clockDomain:'reference:dart',probes:pair(1000000).start},
        end:{clockDomain:'reference:dart',probes:pair(1000000).end}}},
    serverDiagnostics:[clock('backend','node:runner',1000100),clock('server-rust','native:runner',50),
      span('Server transaction','backend',1100,150,'n1',undefined,'node'),
      span('SQL','backend',1120,50,'n2','n1','node'),span('Server native','server-rust',900,100,'s1')],
    result:{clockCorrelation:{referenceClockDomain:'reference:dart',measurementClockDomain:'measurement',
      nativeClockDomain:'native:dart',nativeProbes:pair(500),dartClockDomain:'dart-process',dartProbes:pair(100),
      clockRecords:[clock('measurement','reference:dart',1000)],
      setupSpans:[{...span('Library preparation','reference:dart',800,40,undefined,undefined,'setup'),lane:'Excluded setup'}]},
      clockRecords:[clock('dart:A','dart-process',0),clock('rust:A','native:dart',20),clock('rust:B','native:dart',30)],
      diagnostics:[span('A SDK','dart:A',1300,100,'d1',undefined,'dart','A'),
        span('A apply','rust:A',1100,100,'a1',undefined,'rust','A'),span('B watch','rust:B',1000,80,'b1',undefined,'rust','B')],
      timings:{initializationStartUs:0,storeOpenStartUs:0,storeOpenUs:100,firstVisibleUs:300,allVisibleUs:900,bootstrapCompleteUs:1000},
      snapshots:[{atUs:300,rows:100}]}};
}

test('round-trip bounds contain asymmetric transport without inventing one-way latency',()=>{
  // Remote time = local - 1000; 2us outbound, 80us inbound.
  const p={localSendUs:2000,remoteReceiveUs:1002,remoteSendUs:1005,localReceiveUs:2085};
  assert.deepEqual(probeBounds(p),{lowerUs:997,upperUs:1081});
  const c=calibration('remote','local',[p],'round-trip');
  assert.ok(c.offsetLowerUs<=1000&&c.offsetUpperUs>=1000);
  assert.equal(c.probes[0],p);
});

test('changed offset outside endpoint bounds refuses a fabricated common axis',()=>{
  const a=calibration('a','b',[probe(1000)],'test');
  const b=calibration('a','b',[probe(2000)],'test');
  assert.throws(()=>combineCalibrations(a,b),/offset changed/);
  const s=sample();s.clockCorrelation.dartRunnerProbes.end.probes=[probe(1001000)];
  assert.throws(()=>timeline(s),/offset changed/);
});

test('all lanes retain actual shared-axis start times, overlap, raw clocks and uncertainty',()=>{
  const trace=timeline(sample()), x=name=>trace.traceEvents.find(e=>e.name===name&&e.ph==='X');
  assert.equal(x('Server transaction').ts,200);
  assert.equal(x('SQL').ts,220);
  assert.equal(x('Server native').ts,150);
  assert.equal(x('A SDK').ts,400);
  assert.equal(x('A apply').ts,620);
  assert.equal(x('B watch').ts,530);
  assert.equal(x('Library preparation').ts,-200);
  assert.equal(x('Library preparation').dur,40);
  assert.equal(x('Server transaction').tid,x('SQL').tid);
  assert.equal(x('SQL').args.parentSpanId,'n1');
  assert.equal(x('A apply').args.rawStartUs,1100);
  assert.equal(x('A apply').dur,100);
  assert.ok(x('A apply').args.alignmentUncertaintyUs>0);
  assert.ok(trace.traceEvents.some(e=>e.name==='No recorded source span (unattributed)'));
  assert.ok(trace.clockAlignment.edges.every(e=>e.offsetLowerUs<=e.offsetUpperUs));
});

test('concurrent siblings get separate tracks and never become each other’s parent',()=>{
  const s=sample();s.serverDiagnostics.push(span('Parallel SQL','backend',1140,50,'n3','n1','node'));
  const t=timeline(s), a=t.traceEvents.find(e=>e.name==='SQL'),b=t.traceEvents.find(e=>e.name==='Parallel SQL');
  assert.notEqual(a.tid,b.tid);assert.equal(b.args.parentSpanId,'n1');assert.equal(b.ts,240);
});

test('different raw native epochs are packed according to aligned positions',()=>{
  const s=sample();
  s.serverDiagnostics.push(clock('server-next','native:runner',500),
    span('Later native','server-next',0,100,'next'));
  const t=timeline(s),first=t.traceEvents.find(e=>e.name==='Server native'),later=t.traceEvents.find(e=>e.name==='Later native');
  assert.equal(first.ts,150);assert.equal(later.ts,-300);
  assert.equal(first.tid,later.tid,'nonoverlapping real intervals share a lane despite different raw epochs');
});

test('old archives, missing parents and loss are never silently converted to exact traces',()=>{
  const old=sample();delete old.clockCorrelation;assert.throws(()=>timeline(old),/no clock calibration/);
  const orphan=sample();orphan.result.diagnostics[0].parentSpanId='unknown';assert.throws(()=>timeline(orphan),/missing parent/);
  const lossy=sample();lossy.result.diagnostics.push({kind:'loss'});assert.throws(()=>timeline(lossy),/trace loss/);
});
