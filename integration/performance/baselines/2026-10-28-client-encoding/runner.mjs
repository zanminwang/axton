import assert from 'node:assert/strict';
import {createHash,randomBytes} from 'node:crypto';
import {execFileSync,spawn} from 'node:child_process';
import {readFile,writeFile,mkdir,mkdtemp,rm} from 'node:fs/promises';
import {tmpdir,cpus,totalmem,platform,release,arch,loadavg} from 'node:os';
import {dirname,resolve,join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createInterface} from 'node:readline';
import {createRequire} from 'node:module';
import pgModule from '/Users/stevewang/.codex/worktrees/sql-roundtrips/axton/node_modules/pg/lib/index.js';
const {Pool}=pgModule;
import {startFixture,textBytes,fixtureBatchSize} from '/Users/stevewang/.codex/worktrees/sql-roundtrips/axton/integration/performance/fixture/server.mjs';
import {printSummary,summarize,metrics,validateRecordReports,serverSettlements} from '/Users/stevewang/.codex/worktrees/sql-roundtrips/axton/integration/performance/report.mjs';
import {poolLifecycle} from '/Users/stevewang/.codex/worktrees/sql-roundtrips/axton/integration/performance/support/postgres.mjs';
import {nowUs,runnerClockDomain,sampleNativeClock,retainClockRecords,ClockHandshake} from '/Users/stevewang/.codex/worktrees/sql-roundtrips/axton/integration/performance/support/clock.mjs';
import {timeline} from '/Users/stevewang/.codex/worktrees/sql-roundtrips/axton/integration/performance/timeline.mjs';

const root='/Users/stevewang/.codex/worktrees/sql-roundtrips/axton';
process.chdir(root);
const command=(name,args=[])=>execFileSync(name,args,{encoding:'utf8'}).trim();
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const artifactPaths=['bindings/node/axton-node.node',`target/release/libaxton_dart.${platform()==='darwin'?'dylib':'so'}`,
  'artifacts/performance/initial-sync','artifacts/performance/live-sync'];
async function sourceHash() {
  const paths=command('git',['ls-files','--cached','--others','--exclude-standard','-z']).split('\0').filter(Boolean)
    .filter(path=>!path.startsWith('integration/performance/baselines/')).sort();
  const digest=createHash('sha256');
  for (const path of paths) {
    digest.update(path+'\0');
    try { digest.update(await readFile(path)); } catch(error) { if (error.code!=='ENOENT') throw error; digest.update('<deleted>'); }
  }
  return digest.digest('hex');
}
async function buildIdentity() {
  return {sourceSha256:await sourceHash(),artifacts:Object.fromEntries(await Promise.all(artifactPaths.map(async path=>[path,hash(await readFile(path))])))};
}
if (process.argv.includes('--record-build')) {
  await writeFile('artifacts/performance/build.json',JSON.stringify(await buildIdentity(),null,2)+'\n');
  process.exit(0);
}
const defaults={output:'artifacts/performance/results.json',samples:5,sizes:[100,1000,10000],writes:100,timeout:120000,diagnostics:true,failFast:false};
const options={...defaults};
for (let i=2;i<process.argv.length;i++) {
  const flag=process.argv[i],value=process.argv[++i];
  assert.ok(value,`missing value for ${flag}`);
  if(flag==='--output') options.output=value;
  else if(flag==='--samples') options.samples=Number(value);
  else if(flag==='--sizes') options.sizes=value.split(',').map(Number);
  else if(flag==='--writes') options.writes=Number(value);
  else if(flag==='--timeout-ms') options.timeout=Number(value);
  else if(flag==='--diagnostics') {assert.ok(['on','off'].includes(value));options.diagnostics=value==='on';}
  else if(flag==='--fail-fast') {assert.ok(['on','off'].includes(value));options.failFast=value==='on';}
  else throw Error(`unknown option ${flag}`);
}
for (const value of [options.samples,options.writes,options.timeout,...options.sizes]) assert.ok(Number.isSafeInteger(value)&&value>0,'parameters must be positive integers');
assert.ok(new Set(options.sizes).size===options.sizes.length,'duplicate sizes');
const built=JSON.parse(await readFile('artifacts/performance/build.json','utf8'));
assert.deepEqual(await buildIdentity(),built,'source/artifact changes require a new release build; do not reuse stale artifacts');
const databaseUrl=new URL(process.env.DATABASE_URL);
assert.equal(databaseUrl.hostname,'127.0.0.1','disposable localhost PostgreSQL only');
const admin=new Pool({connectionString:databaseUrl.href});
const adminErrors=[];
const closeAdmin=poolLifecycle(admin,error=>adminErrors.push(String(error)));
let child,fixture,interrupted=false;
const signal=()=>{interrupted=true;child?.kill('SIGTERM');};
process.once('SIGINT',signal);process.once('SIGTERM',signal);
const settings=(await admin.query("SELECT name,setting,unit FROM pg_settings WHERE name IN ('fsync','synchronous_commit','full_page_writes','shared_buffers','wal_level') ORDER BY name")).rows;
const result={schemaVersion:1,completed:false,createdAt:new Date().toISOString(),commit:command('git',['rev-parse','HEAD']),
  dirty:command('git',['status','--porcelain']).length>0,build:built,
  implementation:{publicationFence:'removed; Serializable and Store/record/Stream guards retained',
    bootstrapPrefetch:'enabled in every sample',
    storeBindingReads:'snapshot inspection; atomic claim for missing Store or unfinished Bootstrap',
    storeProgressLock:'FOR NO KEY UPDATE; competing uploads serialized',
    deliveryStorage:'none; complete current-state cursor pages, no server payload tables',
    liveOffers:'idle direct admission even during HTTP wait; staged payloads coalesce to head hints, repair from committed cursor'},
  machine:{platform:platform(),release:release(),arch:arch(),cpu:cpus()[0]?.model,cpus:cpus().length,memoryBytes:totalmem(),
    node:process.version,dart:command(process.env.AXTON_DART??'dart',['--version']),rustc:command('rustc',['--version']),
    cargo:command('cargo',['--version']),typescript:command(process.execPath,['node_modules/typescript/bin/tsc','--version']),
    postgres:(await admin.query('SELECT version() AS version')).rows[0].version,settings},
  compatibility:{schemaVersion:1,stageInstrumentation:3,clockCorrelation:'bounded-monotonic-v1',boundaries:'sync-performance-v2',fixtureSchemaSha256:hash(await readFile('integration/performance/fixture/generated/schema.json')),
    textBytes,fixtureBatchSize,sdk:'dart',nativeProfile:'release',dartMode:'aot',diagnostics:options.diagnostics,
    watch:'full Entry model snapshots; first expected nonempty snapshot',
    initialCondition:'fresh process and SQLite files per sample; native library prepared and clock calibration before timing; application configuration/open/connect/bootstrap timed; process startup excluded',
    cacheCondition:'new PostgreSQL database per sample; fixture just written; OS/PostgreSQL caches are not flushed',
    liveCondition:'two initialized Stores in one process; await each local commit, do not await settlement before next write'},
  parameters:options,adminErrors,samples:[]};
async function save() {
  result.summary=Object.fromEntries([false,true].map(enabled=>[enabled?"on":"off",summarize({...result,samples:result.samples.filter(s=>s.bootstrapPrefetch===enabled)})]));
  await mkdir(dirname(resolve(options.output)),{recursive:true});
  const partial=resolve(options.output)+'.partial';
  await writeFile(partial,JSON.stringify(result,null,2)+'\n');
  const {rename}=await import('node:fs/promises');await rename(partial,resolve(options.output));
}
async function execute(scenario,records,writes,directory,serverDiagnostics) {
  return new Promise(resolveSample=>{
    const progress=[],stderr=[];let output,timeout=false,overflow=false,bytes=0,force;
    const native=options.diagnostics?createRequire(import.meta.url)(resolve(artifactPaths[0])):undefined;
    const clockCorrelation={runnerClockDomain,nodeNativeClockDomain:native?.diagnosticsClockDomain(),
      nodeNativeProbes:options.diagnostics?{start:sampleNativeClock(()=>native.diagnosticsClockUs())}:{},dartRunnerProbes:{}};
    child=spawn(resolve(`artifacts/performance/${scenario}`),[JSON.stringify({scenario,url:fixture.url,records,writes,directory,diagnostics:options.diagnostics})],
      {stdio:['pipe','pipe','pipe'],env:{...process.env,AXTON_DART_LIBRARY:resolve(artifactPaths[1])}});
    const running=child;
    const kill=()=>{running.kill('SIGTERM');force??=setTimeout(()=>running.kill('SIGKILL'),1000);};
    const timer=setTimeout(()=>{timeout=true;kill();},options.timeout);
    const lines=createInterface({input:running.stdout});
    const handshake=new ClockHandshake(line=>running.stdin.write(line));
    clockCorrelation.dartRunnerProbes=handshake.endpoints;
    lines.on('line',line=>{
      const receivedAtUs=nowUs();
      bytes+=Buffer.byteLength(line);
      if(bytes>128*1024*1024){overflow=true;kill();return;}
      try {
        const event=JSON.parse(line);
        if(event.type==='result') output=event;
        else {
          progress.push(event);
          if(!handshake.accept(event,receivedAtUs)&&event.phase==='ready') {retainClockRecords(serverDiagnostics);running.stdin.write('go\n');}
        }
      } catch(error) {progress.push({type:'invalid-output',line:line.slice(0,1000),error:String(error)});}
    });
    running.stderr.on('data',data=>{if(stderr.reduce((n,s)=>n+s.length,0)<1024*1024)stderr.push(String(data));});
    running.on('error',error=>{progress.push({type:'spawn-error',error:String(error)});});
    running.on('close',(code,exitSignal)=>{
      clearTimeout(timer);clearTimeout(force);lines.close();child=undefined;
      if(options.diagnostics)clockCorrelation.nodeNativeProbes.end=sampleNativeClock(()=>native.diagnosticsClockUs());
      const errorProgress=progress.some(e=>e.phase==='connection.error'||e.type==='invalid-output'||e.type==='spawn-error');
      const status=timeout?'timeout':interrupted?'interrupted':overflow?'output-overflow':code===0&&output?.status==='passed'&&!errorProgress?'passed':'failed';
      resolveSample({status,exitCode:code,signal:exitSignal,result:output??null,progress,clockCorrelation,stderr:stderr.join('')});
    });
  });
}
const sequence=[true];
const ordinals={off:0,on:0};
result.experiment={purpose:'Short client canonical-encoding comparison; one isolated sample per invocation',clientVariant:process.env.AXTON_CLIENT_ENCODING_VARIANT,sequence,
  runnerSha256:hash(await readFile(fileURLToPath(import.meta.url))),
  selection:'Fresh N=10000 initial sync; prefetch enabled; backend and AOT artifacts fixed; old/new release Dart libraries with identical timing spans'};
try {
  scenarios:
  for (const [sequenceIndex,enabled] of sequence.entries()) {
        const scenario='initial-sync',records=10000,writes=0,repetition=++ordinals[enabled?'on':'off'];
        process.env.AXTON_EXPERIMENTAL_MUTATION_GROUP_SIZE='8';
        process.env.AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=enabled?'1':'0';
        if(interrupted) break scenarios;
        const sample={scenario,records,writes,repetition,bootstrapPrefetch:enabled,sequenceIndex,status:'setup-failed',loadAverage:loadavg(),serverDiagnostics:[]};
        const database=`performance_${randomBytes(6).toString('hex')}`;
        const directory=await mkdtemp(join(process.env.AXTON_PERFORMANCE_TEMP??tmpdir(),'axton-performance-client-'));
        let created=false,dropped=0;
        try {
          await admin.query(`CREATE DATABASE "${database}"`);created=true;
          const url=new URL(databaseUrl);url.pathname='/'+database;
          fixture=await startFixture(url.href,records,options.diagnostics?{record(record){
            if(sample.serverDiagnostics.length<200000)sample.serverDiagnostics.push(record);else dropped++;
            if(record.kind==='loss')dropped+=Number(record.attributes.records??1);
          }}:undefined);
          retainClockRecords(sample.serverDiagnostics);
          Object.assign(sample,await execute(scenario,records,writes,directory,sample.serverDiagnostics));
          // Final-state evidence is useful even when the child recovered from a
          // connection error. Such a sample still fails; never hide that error.
          if(sample.result?.status==='passed') {
            validateRecordReports(sample);
            if(scenario==='live-sync'&&options.diagnostics) {
              sample.serverSettlements=serverSettlements(sample);
              assert.equal(sample.serverSettlements.length,writes,'one source-clock server commit confirmation per write');
            }
            for (const values of Object.values(metrics(sample)))
              assert.ok(values.every(value=>Number.isFinite(value)&&value>=0),'invalid timing observation');
            sample.serverVerification=await fixture.verify(writes);assert.equal(dropped,0,'server diagnostics incomplete');
            sample.serverVerified=true;
          }
          if(sample.result) {
            try {
              const trace=timeline(sample);
              const path=resolve(options.output+'.traces',`${enabled?"on":"off"}-${scenario}-N${records}-M${writes}-r${repetition}.trace.json`);
              await mkdir(dirname(path),{recursive:true});await writeFile(path,JSON.stringify(trace)+'\n');
              sample.timeline={status:'aligned',path,clockAlignment:trace.clockAlignment,unattributedIntervals:trace.unattributedIntervals};
            } catch(error) {
              sample.timeline={status:'unavailable',reason:String(error)};
              sample.status='failed';sample.error??='common-axis trace unavailable (see timeline.reason)';
            }
          }
        } catch(error) {sample.status='failed';sample.error=String(error);}
        finally {
          try {await fixture?.close();} catch(error) {sample.status='failed';sample.cleanupError=String(error);}
          sample.backendErrors=fixture?.errors.slice()??[];
          if(sample.backendErrors.length) {sample.status='failed';sample.error??='backend/connection errors (see backendErrors)';}
          fixture=undefined;
          if(created) try {await admin.query(`DROP DATABASE "${database}" WITH (FORCE)`);} catch(error) {sample.status='failed';sample.databaseCleanupError=String(error);}
          await rm(directory,{recursive:true,force:true});
          sample.serverDiagnosticsDropped=dropped;
          result.samples.push(sample);await save();
        }
        console.error(`prefetch=${enabled?"on":"off"} ${scenario} N=${records} M=${writes} repetition=${repetition}: ${sample.status}`);
        if(sample.status!=='passed')console.error(JSON.stringify({error:sample.error,cleanupError:sample.cleanupError,client:sample.result?.error,progress:sample.progress}));
        if(sample.status!=='passed'&&options.failFast) break scenarios;
      }
} finally {
  child?.kill('SIGKILL');await fixture?.close();await closeAdmin();
  const expected=sequence.length;
  result.matrixCompleted=!interrupted&&result.samples.length===expected;
  result.completed=result.matrixCompleted&&adminErrors.length===0&&result.samples.every(s=>s.status==='passed');
  await save();
  process.removeListener('SIGINT',signal);process.removeListener('SIGTERM',signal);
}
for(const enabled of [false,true]) {console.log(enabled?"PREFETCH ON":"PREFETCH OFF");printSummary({...result,samples:result.samples.filter(s=>s.bootstrapPrefetch===enabled)});}
console.log(`JSON: ${resolve(options.output)}`);
if(!result.completed)process.exitCode=1;
