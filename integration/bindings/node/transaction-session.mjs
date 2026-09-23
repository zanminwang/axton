import { createRequire } from 'node:module';
const { runProbe } = createRequire(import.meta.url)('../../../bindings/node/axton-node-probe.node');
const scopes = new WeakMap();

/** Test fixture over the probe-only addon build; only an active caller-owned transaction can bind it. */
export class TransactionProbe {
  #tx; #scope; #closed = false; #failed = false; #pending = 0; #beforeCallback;
  constructor(tx, {beforeCallback = async () => {}} = {}) {
    const scope = scopes.get(tx);
    if (!scope || !scope.open) throw Error('transaction_closed');
    this.#tx = tx;
    this.#scope = scope;
    this.#beforeCallback = beforeCallback;
    scope.sessions.add(this);
  }
  #assertOpen() {
    if (this.#closed || !this.#scope.open) throw Error('transaction_closed');
    if (this.#failed) throw Error('transaction_failed');
  }
  async run(id, {failAfterWrite = false} = {}) {
    this.#assertOpen();
    this.#pending++;
    try {
      const result = await runProbe(async operation => {
        this.#assertOpen();
        await this.#beforeCallback(operation);
        this.#assertOpen();
        if (operation === 'write') {
          await this.#tx.frameworkProbe.create({data:{id}});
          return 0;
        }
        if (operation === 'count') return this.#tx.frameworkProbe.count();
        throw Error('unknown_host_operation');
      }, failAfterWrite);
      this.#assertOpen();
      return result;
    } catch (error) {
      this.#failed = true;
      throw error;
    } finally { this.#pending--; }
  }
  assertCommittable() {
    if (this.#failed || this.#pending) throw Error('transaction_failed');
  }
  close() { this.#closed = true; }
}

/** The supplied runner owns BEGIN/COMMIT/ROLLBACK. Results leave only after COMMIT. */
export async function committedResult(transactionRunner, body) {
  return transactionRunner(async tx => {
    const scope = {open:true, sessions:new Set()};
    if (scopes.has(tx)) throw Error('transaction_already_bound');
    scopes.set(tx, scope);
    try {
      const result = await body(tx);
      for (const session of scope.sessions) session.assertCommittable();
      return result;
    } finally {
      scope.open = false;
      for (const session of scope.sessions) session.close();
      scopes.delete(tx);
    }
  });
}
