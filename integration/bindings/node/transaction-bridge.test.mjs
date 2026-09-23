import { test, before, after, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { setTimeout as delay } from 'node:timers/promises';
import { TransactionProbe, committedResult } from './transaction-session.mjs';
const require = createRequire(import.meta.url);
const { PrismaClient } = require('./generated/client');
const prisma = new PrismaClient();
before(async () => { await prisma.$executeRawUnsafe('CREATE TABLE IF NOT EXISTS "BusinessProbe" (id TEXT PRIMARY KEY)'); await prisma.$executeRawUnsafe('CREATE TABLE IF NOT EXISTS "FrameworkProbe" (id TEXT PRIMARY KEY)'); });
beforeEach(async () => { await prisma.frameworkProbe.deleteMany(); await prisma.businessProbe.deleteMany(); });
after(() => prisma.$disconnect());
const runner = (body) => prisma.$transaction(body, {isolationLevel:'RepeatableRead'});
async function rows() { return [await prisma.businessProbe.count(), await prisma.frameworkProbe.count()]; }

test('native Rust awaits asynchronous host writes and reads in user transaction; outer rollback removes both', async () => {
  await assert.rejects(committedResult(runner, async tx => {
    await tx.businessProbe.create({data:{id:'rollback'}});
    const session = new TransactionProbe(tx);
    const result = await session.run('rollback');
    assert.equal(result.observed, 1);
    assert.equal(result.callbackCount, 2);
    assert.equal(result.payloadBytes, 18);
    assert.ok(result.elapsedMicros >= 0);
    session.close();
    throw Error('force rollback');
  }), /force rollback/);
  assert.deepEqual(await rows(), [0,0]);
});

test('rollback assertion detects accidentally global Prisma writes', async () => {
  await assert.rejects(prisma.$transaction(async tx => {
    await tx.businessProbe.create({data:{id:'negative-control'}});
    await prisma.frameworkProbe.create({data:{id:'negative-control'}});
    throw Error('rollback');
  }), /rollback/);
  assert.deepEqual(await rows(), [0,1]);
});

test('successful commit is the gate for the accepted result and closes saved handles', async () => {
  let saved;
  const result = await committedResult(runner, async tx => {
    saved = new TransactionProbe(tx);
    await tx.businessProbe.create({data:{id:'commit'}});
    await saved.run('commit');
    return {accepted:true};
  });
  assert.deepEqual(result, {accepted:true});
  assert.deepEqual(await rows(), [1,1]);
  await assert.rejects(saved.run('late'), /transaction_closed/);
  assert.deepEqual(await rows(), [1,1]);
});

test('callback rejection poisons session even if application catches it', async () => {
  await assert.rejects(committedResult(runner, async tx => {
    await tx.businessProbe.create({data:{id:'callback'}});
    const session = new TransactionProbe(tx, {beforeCallback: async () => { throw Error('callback rejected'); }});
    await assert.rejects(session.run('callback'), /callback rejected/);
    return {accepted:true};
  }), /transaction_failed/);
  assert.deepEqual(await rows(), [0,0]);
});

test('Rust error after host write rolls back business and framework', async () => {
  await assert.rejects(committedResult(runner, async tx => {
    await tx.businessProbe.create({data:{id:'rust'}});
    await new TransactionProbe(tx).run('rust', {failAfterWrite:true});
  }), /rust_probe_error/);
  assert.deepEqual(await rows(), [0,0]);
});

test('dispose during awaited callback rejects and rolls back', async () => {
  await assert.rejects(committedResult(runner, async tx => {
    const session = new TransactionProbe(tx, {beforeCallback: async () => { session.close(); await delay(5); }});
    await tx.businessProbe.create({data:{id:'dispose'}});
    await session.run('dispose');
  }), /transaction_closed/);
  assert.deepEqual(await rows(), [0,0]);
});

test('concurrent transactions cannot share handles or read each other uncommitted rows', async () => {
  const result = await Promise.all(['a','b'].map(id => committedResult(runner, async tx => {
    await tx.businessProbe.create({data:{id}});
    return new TransactionProbe(tx, {beforeCallback: () => delay(5)}).run(id);
  })));
  assert.deepEqual(result.map(r => r.observed), [1,1]);
  assert.deepEqual(await rows(), [2,2]);
});

test('transaction timeout closes handle and yields no accepted result', async () => {
  let saved;
  await assert.rejects(committedResult(body => prisma.$transaction(body,{timeout:40}), async tx => {
    saved = new TransactionProbe(tx, {beforeCallback: () => delay(100)});
    await tx.businessProbe.create({data:{id:'timeout'}});
    await saved.run('timeout');
    return {accepted:true};
  }));
  await assert.rejects(saved.run('late'), /transaction_closed/);
  assert.deepEqual(await rows(), [0,0]);
});

test('real deferred constraint commit failure never returns accepted result', async () => {
  await prisma.$executeRawUnsafe('CREATE TABLE IF NOT EXISTS "CommitProbe" (id TEXT PRIMARY KEY, parent TEXT REFERENCES "BusinessProbe"(id) DEFERRABLE INITIALLY DEFERRED)');
  let saved;
  await assert.rejects(committedResult(runner, async tx => {
    saved = new TransactionProbe(tx);
    await saved.run('commit-fail');
    await tx.$executeRaw`INSERT INTO "CommitProbe" (id,parent) VALUES ('bad','missing')`;
    return {accepted:true};
  }));
  await assert.rejects(saved.run('late'), /transaction_closed/);
  assert.deepEqual(await rows(), [0,0]);
});

test('native boundary captures synchronous callback throws without terminating Node', async () => {
  const {runProbe} = require('../../../bindings/node/axton-node-probe.node');
  await assert.rejects(runProbe(() => { throw Error('synchronous callback'); }, false), /synchronous callback/);
});

test('unawaited work prevents commit and saved operation rejects after closure', async () => {
  let work;
  await assert.rejects(committedResult(runner, async tx => {
    await tx.businessProbe.create({data:{id:'unawaited'}});
    work = new TransactionProbe(tx,{beforeCallback: () => delay(20)}).run('unawaited');
    work.catch(() => {});
    return {accepted:true};
  }), /transaction_failed/);
  await assert.rejects(work, /transaction_closed/);
  assert.deepEqual(await rows(), [0,0]);
});

test('business rejection rolls back its savepoint while preceding mutation commits', async () => {
  await committedResult(runner, async tx => {
    const session = new TransactionProbe(tx);
    await tx.$executeRawUnsafe('SAVEPOINT mutation_one');
    await tx.businessProbe.create({data:{id:'one'}});
    await session.run('one');
    await tx.$executeRawUnsafe('RELEASE SAVEPOINT mutation_one');
    await tx.$executeRawUnsafe('SAVEPOINT mutation_two');
    await tx.businessProbe.create({data:{id:'two'}});
    await session.run('two');
    // Domain rejection is a value, not an unexpected bridge/persistence error.
    await tx.$executeRawUnsafe('ROLLBACK TO SAVEPOINT mutation_two');
    await tx.$executeRawUnsafe('RELEASE SAVEPOINT mutation_two');
  });
  assert.deepEqual((await prisma.businessProbe.findMany()).map(r => r.id), ['one']);
  assert.deepEqual((await prisma.frameworkProbe.findMany()).map(r => r.id), ['one']);
});
