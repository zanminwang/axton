import test from 'node:test';
import assert from 'node:assert/strict';
import {statistics,summarize,compare,validateRecordReports} from './report.mjs';
test('median/range and nearest-rank p95 only with at least 100 observations',()=>{
  assert.deepEqual(statistics([9,1,3,2]),{n:4,median:2.5,min:1,max:9});
  assert.equal(statistics(Array.from({length:100},(_,i)=>i+1)).p95,95);
  assert.throws(()=>statistics([NaN]));
});
const sample={scenario:'initial-sync',records:100,writes:0,status:'passed',serverDiagnostics:[],result:{diagnostics:[],
  timings:{initializationStartUs:100,storeOpenStartUs:100,storeOpenUs:1100,firstVisibleUs:3100,allVisibleUs:4100,bootstrapCompleteUs:5100}}};
const result={completed:true,compatibility:{version:1},machine:{cpu:'test'},samples:[sample]};
test('failed samples remain visible and never enter latency statistics',()=>{
  const output=summarize({...result,samples:[sample,{...sample,status:'timeout',result:null}]})[0];
  assert.equal(output.passed,1);assert.equal(output.failed,1);assert.equal(output.metrics.storeOpenMs.median,1);
});
test('comparison rejects changed boundaries, environment, missing scenarios and failures',()=>{
  assert.ok(compare(result,result).every(x=>x.changePercent===0));
  for (const changed of [{...result,completed:false},{...result,compatibility:{}},{...result,machine:{}},{...result,samples:[]},
    {...result,samples:[{...sample,status:'failed'}]}]) assert.throws(()=>compare(result,changed));
});
test('only the known own-create transient report is admitted after accepted-state verification',()=>{
  const event={id:'live-00000000',accepted:true};
  const report={client:'A',kind:'diverged',model:'Entry',identity:{id:event.id},ordinal:1,duringMeasurement:true};
  const sample={scenario:'live-sync',result:{events:[event],recordReports:[report]}};
  assert.doesNotThrow(()=>validateRecordReports(sample));
  for(const changed of [{...report,client:'B'},{...report,kind:'skipped'},{...report,ordinal:2},
    {...report,duringMeasurement:false},{...report,identity:{id:'unrelated'}}])
    assert.throws(()=>validateRecordReports({...sample,result:{events:[event],recordReports:[changed]}}));
  assert.throws(()=>validateRecordReports({...sample,result:{events:[{...event,accepted:false}],recordReports:[report]}}));
});
