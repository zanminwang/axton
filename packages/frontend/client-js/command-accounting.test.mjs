import test from 'node:test';
import assert from 'node:assert/strict';
import { Transaction as NodeTransaction } from './api/transaction.mts';
import { Transaction as MobileTransaction } from '../client-react-native/api/transaction.mts';

const adapters = [
  ['Node', (send, body) => runTransaction(NodeTransaction, send, body)],
  ['RN', (send, body) => runTransaction(MobileTransaction, send, body)],
];

async function runTransaction(Type, send, body) {
  const tx = new Type(send);
  await body(tx);
  await tx.finish();
}

for (const [name, run] of adapters) {
  test(`${name}: awaited commands return results and expired handles refuse work`, async () => {
    const sent = [];
    let handle;
    await run(async command => { sent.push(command); return { text: 'read' }; }, async tx => {
      handle = tx;
      assert.deepEqual(await tx.read('Entry', { id: 'e' }), { text: 'read' });
      await tx.query('Entry', { id: 'e' });
    });
    assert.deepEqual(sent.map(command => command.kind), ['read', 'query']);
    await assert.rejects(handle.read('Entry', { id: 'e' }), /closed/);
    assert.equal(sent.length, 2);
  });

  test(`${name}: synchronous command failure remains the original finish failure`, async () => {
    const failure = Error('synchronous send failure');
    await assert.rejects(run(() => { throw failure; }, async tx => {
      await assert.rejects(tx.read('Entry', { id: 'e' }), error => error === failure);
    }), error => error === failure);
  });

  test(`${name}: swallowed command failures preserve the first error`, async () => {
    const first = Error('first command failure');
    const second = Error('second command failure');
    let sends = 0;
    await assert.rejects(run(async () => { throw ++sends === 1 ? first : second; }, async tx => {
      await tx.read('Entry', { id: 'e' }).catch(() => {});
      await tx.read('Entry', { id: 'e' }).catch(() => {});
    }), error => error === first);
    assert.equal(sends, 2);
  });

  test(`${name}: finish drains pending work before refusing unawaited commands`, async () => {
    let release;
    let started;
    const gate = new Promise(resolve => { release = resolve; });
    const entered = new Promise(resolve => { started = resolve; });
    let completed = false;
    const ending = run(async () => { await gate; completed = true; }, async tx => {
      void tx.read('Entry', { id: 'e' });
      started();
    });
    let settled = false;
    const refused = assert.rejects(ending, /unawaited transaction operation/).then(() => { settled = true; });
    await entered;
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(settled, false);
    release();
    await refused;
    assert.equal(completed, true);
  });
}

for (const [name, Type] of [['Node', NodeTransaction], ['RN', MobileTransaction]]) {
  test(`${name}: cancellation expires commands while finish drains existing work`, async () => {
    let release;
    const gate = new Promise(resolve => { release = resolve; });
    let sends = 0;
    const tx = new Type(async () => { sends++; await gate; });
    const work = tx.read('Entry', { id: 'e' });
    tx.cancel();
    await assert.rejects(tx.read('Entry', { id: 'e' }), /transaction_closed/);
    const refused = assert.rejects(tx.finish(), /unawaited transaction operation/);
    release();
    await work;
    await refused;
    assert.equal(sends, 1);
  });
}

test('Node: savepoint rollback restores outer failure accounting for later work', async () => {
  const failure = Error('inner command failed');
  const sent = [];
  const tx = new NodeTransaction(async command => {
    sent.push(command.kind);
    if (command.kind === 'savepoint') return { scope: 'inner' };
    if (command.kind === 'query') throw failure;
    return { text: 'after rollback' };
  });
  await assert.rejects(tx.savepoint(async () => {
    await tx.query('Entry').catch(() => {});
  }), error => error === failure);
  assert.deepEqual(await tx.read('Entry', { id: 'e' }), { text: 'after rollback' });
  await tx.finish();
  assert.deepEqual(sent, ['savepoint', 'query', 'rollbackSavepoint', 'read']);
});
