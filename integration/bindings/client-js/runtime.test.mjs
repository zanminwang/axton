import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Client } from '../../../packages/client-js/index.mts';

const schema = JSON.parse(await readFile(new URL('../../../fixtures/schemas/entry.json', import.meta.url), 'utf8'));
const edit = (id, text) => ({ name: 'Edit', operations: [{ model: 'Entry', op: 'update', identity: { id }, values: { text } }] });
async function within(promise, ms) {
 let timeout;
 try {
  return await Promise.race([promise, new Promise(resolve => { timeout = setTimeout(() => resolve('timeout'), ms); })]);
 } finally { clearTimeout(timeout); }
}

test('a captured client mutate rejects promptly inside its transaction callback', async () => {
 const dir = await mkdtemp(join(tmpdir(), 'axton-runtime-'));
 const client = await Client.open({ path: join(dir, 'client.sqlite'), schema });
 let release, entered;
 const gate = new Promise(resolve => { release = resolve; });
 const started = new Promise(resolve => { entered = resolve; });
 let attempt;
 try {
  await client.transaction(tx => tx.direct({ model: 'Entry', op: 'create', identity: { id: 'e' }, values: { text: 'A' } }));
  const capturedMutate = client.mutate.bind(client);
  const transaction = client.transaction(async () => {
   attempt = capturedMutate(edit('e', 'B')).then(() => 'accepted', error => error.message);
   entered();
   await gate;
  });
  try {
   await started;
   assert.equal(await within(attempt, 1000), 'transaction_active');
  } finally {
   release();
   await transaction;
  }
  assert.equal((await client.syncState()).pending, 0);
  assert.equal(await client.mutate(edit('e', 'C')), 1);
  assert.equal((await client.read('Entry', { id: 'e' })).text, 'C');
 } finally { await client.close(); await rm(dir, { recursive: true, force: true }); }
});

test('an unrelated Node async context waits for a transaction then enqueues', async () => {
 const dir = await mkdtemp(join(tmpdir(), 'axton-runtime-'));
 const client = await Client.open({ path: join(dir, 'client.sqlite'), schema });
 let release, entered;
 const gate = new Promise(resolve => { release = resolve; });
 const started = new Promise(resolve => { entered = resolve; });
 try {
  await client.transaction(tx => tx.direct({ model: 'Entry', op: 'create', identity: { id: 'e' }, values: { text: 'A' } }));
  const transaction = client.transaction(async tx => { entered(); assert.equal('mutate' in tx, false); await gate; });
  await started;
  const mutation = client.mutate(edit('e', 'B'));
  assert.equal(await within(mutation.then(() => 'committed'), 50), 'timeout');
  release();
  await transaction;
  assert.equal(await mutation, 1);
  assert.equal((await client.read('Entry', { id: 'e' })).text, 'B');
 } finally { release?.(); await client.close(); await rm(dir, { recursive: true, force: true }); }
});

test('failed standalone enqueue rolls back its queue entry and optimistic record', async () => {
 const dir = await mkdtemp(join(tmpdir(), 'axton-runtime-'));
 const client = await Client.open({ path: join(dir, 'client.sqlite'), schema });
 try {
  await assert.rejects(client.mutate({ name: 'Broken', operations: [
   { model: 'Entry', op: 'create', identity: { id: 'failed' }, values: { text: 'optimistic' } },
   { model: 'Missing', op: 'create', identity: { id: 'missing' }, values: { text: 'invalid' } },
  ] }));
  assert.equal((await client.syncState()).pending, 0);
  assert.equal(await client.read('Entry', { id: 'failed' }), null);
  assert.equal(await client.mutate({ name: 'Create', operations: [
   { model: 'Entry', op: 'create', identity: { id: 'good' }, values: { text: 'committed' } },
  ] }), 1);
 } finally { await client.close(); await rm(dir, { recursive: true, force: true }); }
});

test('standalone queued mutation and optimistic value survive SQLite reopen', async () => {
 const dir = await mkdtemp(join(tmpdir(), 'axton-runtime-'));
 const path = join(dir, 'client.sqlite');
 let client;
 try {
  client = await Client.open({ path, schema });
  assert.equal(await client.mutate({ name: 'Create', operations: [
   { model: 'Entry', op: 'create', identity: { id: 'queued' }, values: { text: 'persistent' } },
  ] }), 1);
  await client.close();
  client = await Client.open({ path, schema });
  assert.equal((await client.syncState()).pending, 1);
  assert.equal((await client.read('Entry', { id: 'queued' })).text, 'persistent');
 } finally { await client?.close(); await rm(dir, { recursive: true, force: true }); }
});
