import { AsyncLocalStorage } from "node:async_hooks";
import { type RecordValue, type QuerySpec } from "./values.mts";
export { strictJson, type RecordValue, type QuerySpec } from "./values.mts";
/** Calls execute in submission order and cannot outlive the caller-owned transaction. */
export class Transaction {
  #send: (request: RecordValue) => Promise<any>;
  #open = true;
  #tail: Promise<unknown> = Promise.resolve();
  #pending = 0;
  #failure: unknown;
  #structural: unknown;
  #context = new AsyncLocalStorage<symbol>();
  #publicContext = new AsyncLocalStorage<symbol>();
  #publicToken = Symbol();
  #active: symbol | undefined;
  #scopes = new Set<Promise<unknown>>();
  constructor(send: (request: RecordValue) => Promise<any>) {
    this.#send = send;
  }
  async runCallback<T>(body: () => Promise<T>): Promise<T> {
    try {
      return await this.#publicContext.run(this.#publicToken, body);
    } finally {
      this.#publicContext.disable();
    }
  }
  inCallback(): boolean {
    return this.#publicContext.getStore() === this.#publicToken;
  }
  #queue(request: RecordValue): Promise<any> {
    this.#pending++;
    const work = this.#tail.then(() =>
      this.#send({ ...request, transaction: true }),
    );
    this.#tail = work.then(
      () => {
        this.#pending--;
      },
      (error) => {
        this.#pending--;
        this.#failure ??= error;
      },
    );
    return work;
  }
  #call(request: RecordValue): Promise<any> {
    if (!this.#open) return Promise.reject(Error("transaction_closed"));
    if (this.#active && this.#context.getStore() !== this.#active) {
      this.#structural = Error("overlapping savepoint work");
      return Promise.reject(this.#structural);
    }
    return this.#queue(request);
  }
  async finish(): Promise<void> {
    const outstanding = this.#pending > 0 || this.#scopes.size > 0;
    this.#open = false;
    await this.#tail;
    if (this.#structural) throw this.#structural;
    if (outstanding) throw Error("unawaited transaction operation");
    if (this.#failure) throw this.#failure;
  }
  read(model: string, identity: object): Promise<RecordValue | null> {
    return this.#call({ op: "read", key: { model, identity } });
  }
  query(model: string, where: RecordValue = {}): Promise<RecordValue[]> {
    return this.#call({ op: "query", model, filter: where });
  }
  readSql(sql: string, parameters: unknown[] = []): Promise<RecordValue[]> {
    return this.#call({ op: "sql", sql, parameters });
  }
  querySpec(model: string, query: QuerySpec = {}): Promise<RecordValue[]> {
    return this.#call({ op: "querySpec", model, query });
  }
  related(
    model: string,
    identity: object,
    relation: string,
  ): Promise<RecordValue | null> {
    return this.#call({ op: "related", key: { model, identity }, relation });
  }
  referencing(
    model: string,
    identity: object,
    source: string,
    relation: string,
  ): Promise<RecordValue[]> {
    return this.#call({
      op: "referencing",
      key: { model, identity },
      source,
      relation,
    });
  }
  direct(operation: object) {
    return this.#call({ op: "direct", operation });
  }
  savepoint<T>(body: () => Promise<T>): Promise<T> {
    if (!this.#open) return Promise.reject(Error("transaction_closed"));
    if (this.#active && this.#context.getStore() !== this.#active) {
      this.#structural = Error("overlapping savepoints");
      return Promise.reject(this.#structural);
    }
    const parent = this.#active;
    const token = Symbol();
    this.#active = token;
    const failure = this.#failure;
    const run = this.#context.run(token, async () => {
      await this.#queue({ op: "savepoint" });
      try {
        if (!this.#open) throw Error("transaction_closed");
        const value = await body();
        await this.#tail;
        if (!this.#open) throw Error("transaction_closed");
        if (this.#active !== token) {
          this.#structural = Error("unawaited nested savepoint");
          throw this.#structural;
        }
        if (this.#failure !== failure) throw this.#failure;
        if (this.#structural) throw this.#structural;
        await this.#queue({ op: "release" });
        return value;
      } catch (error) {
        await this.#tail;
        if (this.#open && !this.#structural) {
          if (this.#active !== token) {
            this.#structural = Error("unawaited nested savepoint");
            throw this.#structural;
          }
          await this.#queue({ op: "rollbackSavepoint" });
          this.#failure = failure;
        }
        throw error;
      } finally {
        if (this.#active === token) this.#active = parent;
      }
    });
    this.#scopes.add(run);
    void run.then(
      () => this.#scopes.delete(run),
      () => this.#scopes.delete(run),
    );
    return run;
  }
}
