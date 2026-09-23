import type { RecordValue, QuerySpec } from "../client-js/values.mts";
/** Calls execute in submission order and cannot outlive the caller-owned transaction. */
export class Transaction {
  #send: (request: RecordValue) => Promise<any>;
  #open = true;
  #tail: Promise<unknown> = Promise.resolve();
  #pending = 0;
  #failure: unknown;
  #activeCallback = false;
  constructor(send: (request: RecordValue) => Promise<any>) {
    this.#send = send;
  }
  async runCallback<T>(body: () => Promise<T>): Promise<T> {
    this.#activeCallback = true;
    try {
      return await body();
    } finally {
      this.#activeCallback = false;
    }
  }
  inCallback(): boolean {
    return this.#activeCallback;
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
    return this.#queue(request);
  }
  async finish(): Promise<void> {
    const outstanding = this.#pending > 0;
    this.#open = false;
    await this.#tail;
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
}
