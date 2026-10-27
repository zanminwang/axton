import test from 'node:test';
import assert from 'node:assert/strict';
import {statistics,summarize,compare,validateRecordReports,serverSettlements} from './report.mjs';
test('median/range and nearest-rank p95 only with at least 100 observations',()=>{
  assert.deepEqual(statistics([9,1,3,2]),{n:4,median:2.5,min:1,max:9});
  assert.equal(statistics(Array.from({length:100},(_,i)=>i+1)).p95,95);
  assert.throws(()=>statistics([NaN]));
});
const sample={scenario:'initial-sync',records:100,writes:0,status:'passed',serverDiagnostics:[],result:{diagnostics:[],
  timings:{initializationStartUs:100,storeOpenStartUs:100,storeOpenUs:1100,firstVisibleUs:3100,allVisibleUs:4100,bootstrapCompleteUs:5100}}};
const result={completed:true,compatibility:{version:1},machine:{cpu:'test'},samples:[sample]};
test('server commit confirmations correlate successful transactions without crossing clocks',()=>{
  const tx=(id,outcome,start=100)=>({source:'node',name:'server.transaction',clockDomain:'server',spanId:id,
    startUs:start,durationUs:20,attributes:{operationId:id,outcome}});
  const action=(operationId)=>({source:'node',name:'host.handleAction',clockDomain:'server',
    attributes:{operationId,callId:'store:7:1',mutation:'CreateEntry',outcome:'success'}});
  const sample={scenario:'live-sync',result:{events:[{id:'live-00000000'}]},
    serverDiagnostics:[tx('failed','error'),action('failed'),tx('ok','success'),action('ok'),tx('repeat','success',200),action('repeat')]};
  assert.deepEqual(serverSettlements(sample),[{id:'live-00000000',ordinal:1,callId:'store:7:1',clockDomain:'server',
    transactionSpanId:'ok',transactionStartUs:100,committedUs:120,transactionDurationUs:20}]);
  assert.deepEqual(serverSettlements({...sample,serverDiagnostics:[]}),[]);
});
test('failed samples remain visible and never enter latency statistics',()=>{
  const output=summarize({...result,samples:[sample,{...sample,status:'timeout',result:null}]})[0];
  assert.equal(output.passed,1);assert.equal(output.failed,1);assert.equal(output.metrics.storeOpenMs.median,1);
  const reports=summarize({...result,samples:[sample,{...sample,status:'failed',result:{recordReports:[{}]}}]})[0];
  assert.equal(reports.recordReports,0);assert.equal(reports.failedRecordReports,1);
});
test('comparison rejects changed boundaries, environment, missing scenarios and failures',()=>{
  assert.ok(compare(result,result).every(x=>x.changePercent===0));
  for (const changed of [{...result,completed:false},{...result,compatibility:{}},{...result,machine:{}},{...result,samples:[]},
    {...result,samples:[{...sample,status:'failed'}]}]) assert.throws(()=>compare(result,changed));
});
test('explicit scenario comparison admits only whole passing cases from fully attempted matrices',()=>{
  const failed={scenario:'live-sync',records:100,writes:1,status:'failed',result:null,serverDiagnostics:[]};
  const mixed={...result,completed:false,matrixCompleted:true,samples:[sample,failed]};
  assert.throws(()=>compare(mixed,mixed));
  assert.ok(compare(mixed,mixed,{scenario:'initial-sync'}).every(x=>x.changePercent===0));
  assert.throws(()=>compare(mixed,mixed,{scenario:'live-sync'}));
  assert.throws(()=>compare({...mixed,matrixCompleted:false},mixed,{scenario:'initial-sync'}));
  assert.throws(()=>compare(mixed,mixed,{scenario:'unknown'}));
});
test('only the known own-create transient report is admitted after accepted-state verification',()=>{
  const event={id:'live-00000000',accepted:true,startUs:10};
  const report={client:'A',kind:'diverged',model:'Entry',identity:{id:event.id},ordinal:1,duringMeasurement:true,atUs:20};
  const sample={scenario:'live-sync',result:{events:[event],recordReports:[report],timings:{allSettledUs:30,allVisibleUs:25}}};
  assert.doesNotThrow(()=>validateRecordReports(sample));
  for(const changed of [{...report,client:'B'},{...report,kind:'skipped'},{...report,ordinal:2},
    {...report,duringMeasurement:false},{...report,atUs:9},{...report,atUs:31},{...report,identity:{id:'unrelated'}}])
    assert.throws(()=>validateRecordReports({...sample,result:{...sample.result,recordReports:[changed]}}));
  assert.throws(()=>validateRecordReports({...sample,result:{...sample.result,events:[{...event,accepted:false}]}}));
});
