import type { RecordValue } from "./values.mts";

/** Copy JSON containers without retaining application-owned mutable values. */
export function copyWatchJson<T>(value: T): T {
  if (Array.isArray(value)) return value.map(copyWatchJson) as T;
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, copyWatchJson(item)]),
    ) as T;
  return value;
}

function integer(value: unknown, minimum: number): value is number {
  return Number.isSafeInteger(value) && (value as number) >= minimum;
}

function record(value: unknown): value is RecordValue {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Rust owns membership/order; this cache only replays its sequential splices. */
export class ModelWatchReceiver<T> {
  #rows: T[] = [];
  #version: number | undefined;
  #recovering = false;
  readonly #decode: (row: RecordValue) => T;
  readonly #copy: (value: T) => T;

  constructor(decode: (row: RecordValue) => T, copy: (value: T) => T) {
    this.#decode = decode;
    this.#copy = copy;
  }

  /** Mark a failed delivery; request at most one snapshot until a full reset. */
  recover(): boolean {
    if (this.#recovering) return false;
    this.#recovering = true;
    return true;
  }

  receive(snapshot: RecordValue): T[] | undefined {
    if (snapshot.closed === true) return;
    if (this.#recovering && !("rows" in snapshot)) return;
    const invalid = () => Error("invalid Model watch snapshot");
    if (snapshot.kind !== "watch") throw invalid();
    let version: number | undefined;
    let next: T[];
    if (snapshot.protocol === undefined) {
      if (
        "splices" in snapshot ||
        "version" in snapshot ||
        "baseVersion" in snapshot
      )
        throw invalid();
      next = this.#decodeRows(snapshot.rows);
    } else {
      if (snapshot.protocol !== "splice-v1" || !integer(snapshot.version, 1))
        throw invalid();
      version = snapshot.version;
      if ("rows" in snapshot) {
        if (
          "splices" in snapshot ||
          "baseVersion" in snapshot ||
          (this.#version !== undefined && version <= this.#version)
        )
          throw invalid();
        next = this.#decodeRows(snapshot.rows);
      } else {
        if (
          this.#version === undefined ||
          !integer(snapshot.baseVersion, 1) ||
          snapshot.baseVersion !== this.#version ||
          version !== snapshot.baseVersion + 1 ||
          !Array.isArray(snapshot.splices)
        )
          throw invalid();
        next = this.#rows.slice();
        for (const splice of snapshot.splices) {
          if (
            !record(splice) ||
            !integer(splice.index, 0) ||
            !integer(splice.deleteCount, 0) ||
            splice.index > next.length ||
            splice.deleteCount > next.length - splice.index
          )
            throw invalid();
          const inserted = this.#decodeRows(splice.rows);
          if (inserted.length <= 1) {
            next.splice(splice.index, splice.deleteCount, ...inserted);
          } else {
            // Arbitrarily large batches must not exceed JS argument limits.
            next = next
              .slice(0, splice.index)
              .concat(inserted, next.slice(splice.index + splice.deleteCount));
          }
        }
      }
    }
    // Validate/decode/copy the complete delivery before changing private state.
    const delivered = next.map(this.#copy);
    this.#rows = next;
    this.#version = version;
    this.#recovering = false;
    return delivered;
  }

  #decodeRows(rows: unknown): T[] {
    if (!Array.isArray(rows) || !rows.every(record))
      throw Error("invalid Model watch rows");
    return rows.map(this.#decode);
  }
}
