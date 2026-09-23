import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Client } from '../../../packages/client-js/index.mts';

const schema = JSON.parse(await readFile(new URL('../../../fixtures/schemas/entry.json', import.meta.url), 'utf8'));
const breaking = structuredClone(schema);
breaking.models[0].fields.push({ name: 'due', nullable: false, type: { kind: 'scalar', name: 'string' } });
const exists = p => stat(p).then(() => true, () => false);

test('an incompatible schema keeps unsent work in the old file until it is sent, then rebuild switches files', async () => {
 const dir = await mkdtemp(join(tmpdir(), 'axton-rebuild-'));
 const path = join(dir, 'client.sqlite');
 try {
  let client = await Client.open({ path, schema });
  assert.equal((await client.syncState()).schema.rebuilt, false);
  await client.transaction(tx => tx.direct({ model: 'Entry', op: 'create', identity: { id: 'e' }, values: { text: 'A' } }));
  await client.mutate({ name: 'Edit', operations: [{ model: 'Entry', op: 'update', identity: { id: 'e' }, values: { text: 'B' } }] });
  assert.notEqual(await client.freeze(), null);
  await client.close();
  client = await Client.open({ path, schema: breaking });
  let status = await client.syncState();
  assert.equal(status.schema.rebuilt, false);
  assert.equal(status.schema.pending.pending, 1, 'the old file is kept open for its unsent work');
  assert.match(status.schema.pending.reason, /due/);
  await assert.rejects(() => client.rebuild(), /unsent/);
  const report = await client.rebuild({ discardPending: true });
  assert.equal(report.leftPending, 1);
  assert.match(report.newFile, /client\.sqlite\.1$/);
  status = await client.syncState();
  assert.equal(status.schema.rebuilt, true);
  assert.equal(status.schema.pending, null);
  assert.equal(status.pending, 0);
  assert.equal(await client.read('Entry', { id: 'e' }), null, 'the fresh file is empty');
  assert.equal(await exists(path), true, 'the old file is kept');
  assert.equal(await exists(`${path}.1`), true);
  await client.close();
  // Reopening follows the sidecar to the new file.
  client = await Client.open({ path, schema: breaking });
  assert.equal((await client.syncState()).schema.rebuilt, false);
  await client.close();
 } finally { await rm(dir, { recursive: true, force: true }); }
});
