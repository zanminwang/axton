import { AsyncLocalStorage } from "node:async_hooks";

/** Times are microseconds in one clock domain. Stage names are experimental. */
export interface AxtonTraceRecord {
  readonly schemaVersion: 1;
  readonly kind: "span" | "event" | "loss";
  readonly name: string;
  readonly source: string;
  readonly clockDomain: string;
  readonly spanId?: string;
  readonly parentSpanId?: string | null;
  readonly startUs: number;
  readonly durationUs?: number;
  readonly attributes: Readonly<Record<string, unknown>>;
}
/** Return promptly. Receiver exceptions are ignored, never sync failures. */
export interface AxtonDiagnostics {
  record(record: AxtonTraceRecord): void;
}

let instance = 0;
/** @internal One backend, no process-global tracer or subscriber changes. */
export class Diagnostics {
  private readonly origin = process.hrtime.bigint();
  private readonly clockDomain = `backend:${++instance}`;
  private readonly context = new AsyncLocalStorage<{
    spanId: string;
    operationId: string;
  }>();
  private next = 0;
  private readonly receiver: AxtonDiagnostics | undefined;
  constructor(receiver?: AxtonDiagnostics) {
    this.receiver = receiver;
  }
  /** Preserve this operation across native ThreadsafeFunction callbacks. */
  bind<A extends unknown[], R>(body: (...args: A) => R): (...args: A) => R {
    const context = this.context.getStore();
    return context
      ? (...args) => this.context.run(context, () => body(...args))
      : body;
  }
  record(record: AxtonTraceRecord): void {
    if (!this.receiver) return;
    try {
      this.receiver.record(Object.freeze(record));
    } catch {
      /* Observations never decide outcomes. */
    }
  }
  native(records: AxtonTraceRecord[], dropped: number): void {
    const operationId = this.context.getStore()?.operationId;
    for (const record of records)
      this.record({
        ...record,
        attributes: Object.freeze({
          ...record.attributes,
          ...(operationId ? { operationId } : {}),
        }),
      });
    if (dropped)
      this.record({
        schemaVersion: 1,
        kind: "loss",
        name: "diagnostics.dropped",
        source: "node",
        clockDomain: this.clockDomain,
        startUs: Number((process.hrtime.bigint() - this.origin) / 1000n),
        attributes: Object.freeze({ records: dropped, operationId }),
      });
  }
  span<T>(
    name: string,
    attributes: Record<string, unknown>,
    body: () => Promise<T>,
  ): Promise<T> {
    if (!this.receiver) return body();
    return this.recordedSpan(name, attributes, body);
  }
  private async recordedSpan<T>(
    name: string,
    attributes: Record<string, unknown>,
    body: () => Promise<T>,
  ): Promise<T> {
    const parent = this.context.getStore();
    const spanId = String(++this.next),
      operationId = parent?.operationId ?? `${this.clockDomain}:${spanId}`;
    const start = process.hrtime.bigint();
    let outcome = "success";
    try {
      return await this.context.run({ spanId, operationId }, body);
    } catch (error) {
      outcome = "error";
      throw error;
    } finally {
      this.record({
        schemaVersion: 1,
        kind: "span",
        name,
        source: "node",
        clockDomain: this.clockDomain,
        spanId,
        ...(parent ? { parentSpanId: parent.spanId } : {}),
        startUs: Number((start - this.origin) / 1000n),
        durationUs: Number((process.hrtime.bigint() - start) / 1000n),
        attributes: Object.freeze({ ...attributes, operationId, outcome }),
      });
    }
  }
}
