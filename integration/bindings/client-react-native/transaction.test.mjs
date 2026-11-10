import test from 'node:test';
import assert from 'node:assert/strict';
import * as module from '../../../packages/frontend/client-react-native/api/transaction.mts';

test('mobile transaction adapter is available without a Node async context', () => {
  assert.equal(typeof module?.Transaction, 'function');
});

{
  const {Transaction} = module;
  test('commands serialize and retained scopes reject after finish', async () => {
    const seen = [];
    const scopes = [];
    const tx = new Transaction(async (command, scope) => { seen.push(command); scopes.push(scope); return 1; });
    assert.equal('mutate' in tx, false);
    await tx.query('Entry', {id:'one'});
    await tx.read('Entry', {id:'one'});
    await tx.finish();
    assert.deepEqual(seen.map(x => x.kind), ['query','read']);
    assert.deepEqual(scopes, [undefined, undefined]);
    await assert.rejects(tx.read('Entry', {id:'one'}), /transaction_closed/);
    assert.equal('savepoint' in tx, false);
  });
  test('finish rejects a caught native error', async () => {
    const tx = new Transaction(async () => { throw Error('native failure'); });
    await tx.read('Entry', {id:'e'}).catch(() => {});
    await assert.rejects(tx.finish(), /native failure/);
  });
  test('finish drains unawaited operations before rejecting', async () => {
    let release;
    let completed = false;
    const gate = new Promise(resolve => { release = resolve; });
    const tx = new Transaction(async () => { await gate; completed=true; return 1; });
    const queued = tx.read('Entry', {id:'e'});
    const finish = tx.finish();
    release();
    await assert.rejects(finish, /unawaited transaction operation/);
    await queued;
    assert.equal(completed, true);
  });
}

// Mutations in a transaction through the native runtime (shared with Node):
// the mobile adapter has no savepoints and a coarse callback guard.
import { mutationTests } from '../client-js/mutations-harness.mjs';
mutationTests(test, module.Transaction, { savepoints: false });
