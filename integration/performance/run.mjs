import assert from 'node:assert/strict';
import {createHash,randomBytes} from 'node:crypto';
import {execFileSync,spawn} from 'node:child_process';
import {readFile,writeFile,mkdir,mkdtemp,rm} from 'node:fs/promises';
import {tmpdir,cpus,totalmem,platform,release,arch,loadavg} from 'node:os';
import {dirname,resolve,join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createInterface} from 'node:readline';
import {Pool} from 'pg';
import {startFixture,textBytes,fixtureBatchSize} from './fixture/server.mjs';
import {printSummary,summarize,metrics} from './report.mjs';
import {poolLifecycle} from './support/postgres.mjs';

const root=resolve(dirname(fileURLToPath(import.meta.url)),'../..');
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
const defaults={output:'artifacts/performance/results.json',samples:5,sizes:[100,1000,10000],writes:100,timeout:120000,diagnostics:true};
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
  machine:{platform:platform(),release:release(),arch:arch(),cpu:cpus()[0]?.model,cpus:cpus().length,memoryBytes:totalmem(),
    node:process.version,dart:command(process.env.AXTON_DART??'dart',['--version']),rustc:command('rustc',['--version']),
    cargo:command('cargo',['--version']),typescript:command(process.execPath,['node_modules/typescript/bin/tsc','--version']),
    postgres:(await admin.query('SELECT version() AS version')).rows[0].version,settings},
  compatibility:{schemaVersion:1,boundaries:'sync-performance-v1',fixtureSchemaSha256:hash(await readFile('integration/performance/fixture/generated/schema.json')),
    textBytes,fixtureBatchSize,sdk:'dart',nativeProfile:'release',dartMode:'aot',diagnostics:options.diagnostics,
    watch:'full Entry model snapshots; first expected nonempty snapshot',
    initialCondition:'fresh process and SQLite files per sample; timed interval starts before open, process startup excluded',
    cacheCondition:'new PostgreSQL database per sample; fixture just written; OS/PostgreSQL caches are not flushed',
    liveCondition:'two initialized Stores in one process; await each local commit, do not await settlement before next write'},
  parameters:options,adminErrors,samples:[]};
async function save() {
  result.summary=summarize(result);
  await mkdir(dirname(resolve(options.output)),{recursive:true});
  const partial=resolve(options.output)+'.partial';
  await writeFile(partial,JSON.stringify(result,null,2)+'\n');
  const {rename}=await import('node:fs/promises');await rename(partial,resolve(options.output));
}
async function execute(scenario,records,writes,directory,serverDiagnostics) {
  return new Promise(resolveSample=>{
    const progress=[],stderr=[];let output,timeout=false,overflow=false,bytes=0,force;
    child=spawn(resolve(`artifacts/performance/${scenario}`),[JSON.stringify({scenario,url:fixture.url,records,writes,directory,diagnostics:options.diagnostics})],
      {stdio:['pipe','pipe','pipe'],env:{...process.env,AXTON_DART_LIBRARY:resolve(artifactPaths[1])}});
    const running=child;
    const kill=()=>{running.kill('SIGTERM');force??=setTimeout(()=>running.kill('SIGKILL'),1000);};
    const timer=setTimeout(()=>{timeout=true;kill();},options.timeout);
    const lines=createInterface({input:running.stdout});
    lines.on('line',line=>{
      bytes+=Buffer.byteLength(line);
      if(bytes>128*1024*1024){overflow=true;kill();return;}
      try {
        const event=JSON.parse(line);
        if(event.type==='result') output=event;
        else {
          progress.push(event);
          if(event.phase==='ready') {serverDiagnostics.length=0;running.stdin.end('go\n');}
        }
      } catch(error) {progress.push({type:'invalid-output',line:line.slice(0,1000),error:String(error)});}
    });
    running.stderr.on('data',data=>{if(stderr.reduce((n,s)=>n+s.length,0)<1024*1024)stderr.push(String(data));});
    running.on('error',error=>{progress.push({type:'spawn-error',error:String(error)});});
    running.on('close',(code,exitSignal)=>{
      clearTimeout(timer);clearTimeout(force);lines.close();child=undefined;
      const errorProgress=progress.some(e=>e.phase==='connection.error'||e.type==='invalid-output'||e.type==='spawn-error');
      const status=timeout?'timeout':interrupted?'interrupted':overflow?'output-overflow':code===0&&output?.status==='passed'&&!errorProgress?'passed':'failed';
      resolveSample({status,exitCode:code,signal:exitSignal,result:output??null,progress,stderr:stderr.join('')});
    });
  });
}
try {
  scenarios:
  for (const scenario of ['initial-sync','live-sync']) for (const records of options.sizes)
    for (const writes of scenario==='initial-sync'?[0]:[1,options.writes].filter((n,i,a)=>a.indexOf(n)===i))
      for(let repetition=1;repetition<=options.samples;repetition++) {
        if(interrupted) break scenarios;
        const sample={scenario,records,writes,repetition,status:'setup-failed',loadAverage:loadavg(),serverDiagnostics:[]};
        const database=`performance_${randomBytes(6).toString('hex')}`;
        const directory=await mkdtemp(join(tmpdir(),'axton-performance-client-'));
        let created=false,dropped=0;
        try {
          await admin.query(`CREATE DATABASE "${database}"`);created=true;
          const url=new URL(databaseUrl);url.pathname='/'+database;
          fixture=await startFixture(url.href,records,options.diagnostics?{record(record){
            if(sample.serverDiagnostics.length<200000)sample.serverDiagnostics.push(record);else dropped++;
            if(record.kind==='loss')dropped+=Number(record.attributes.records??1);
          }}:undefined);
          sample.serverDiagnostics.length=0;
          Object.assign(sample,await execute(scenario,records,writes,directory,sample.serverDiagnostics));
          if(sample.status==='passed') {
            for (const values of Object.values(metrics(sample)))
              assert.ok(values.every(value=>Number.isFinite(value)&&value>=0),'invalid timing observation');
            sample.serverVerification=await fixture.verify(writes);assert.equal(dropped,0,'server diagnostics incomplete');
            sample.serverVerified=true;
          }
        } catch(error) {sample.status='failed';sample.error=String(error);}
        finally {
          try {await fixture?.close();} catch(error) {sample.status='failed';sample.cleanupError=String(error);}
          fixture=undefined;
          if(created) try {await admin.query(`DROP DATABASE "${database}" WITH (FORCE)`);} catch(error) {sample.status='failed';sample.databaseCleanupError=String(error);}
          await rm(directory,{recursive:true,force:true});
          sample.serverDiagnosticsDropped=dropped;
          result.samples.push(sample);await save();
        }
        console.error(`${scenario} N=${records} M=${writes} repetition=${repetition}: ${sample.status}`);
        if(sample.status!=='passed') break scenarios;
      }
} finally {
  child?.kill('SIGKILL');await fixture?.close();await closeAdmin();
  const expected=options.sizes.length*(1+new Set([1,options.writes]).size)*options.samples;
  result.completed=!interrupted&&adminErrors.length===0&&result.samples.length===expected&&result.samples.every(s=>s.status==='passed');
  await save();
  process.removeListener('SIGINT',signal);process.removeListener('SIGTERM',signal);
}
printSummary(result);
console.log(`JSON: ${resolve(options.output)}`);
if(!result.completed)process.exitCode=1;
