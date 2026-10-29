import { EngineError } from "@axtonjs/server";
import type {
  Acknowledged,
  HostRequest,
  MemberPosition,
  MemberKey,
  TrackingPair,
  Database,
  Persistence,
} from "@axtonjs/server";
import type { PostgresDriver } from "./driver.mts";
import * as SQL from "./sql.mts";

const safe = (n: unknown): number => {
  const number = Number(n);
  if (!Number.isSafeInteger(number) || number < 0)
    throw new Error("Stored counter outside safe range");
  return number;
};

/** A stored cursor or bound: a safe positive integer. */
const positiveCounter = (n: unknown): number => {
  const number = Number(n);
  if (!Number.isSafeInteger(number) || number < 1)
    throw new Error("Stored counter outside safe positive range");
  return number;
};

/**
 * A Stream name is a string that is non-empty after JS `trim()`. The engine
 * applies its own check (`check_stream` in crates/core, Rust `trim()`) at
 * settlement; the two trims differ on a few code points such as U+FEFF and
 * U+0085, so this is a guard, not the same rule.
 */
const streamName = (stream: unknown): string => {
  if (typeof stream !== "string" || stream.trim() === "")
    throw new Error(`Invalid tracking stream ${JSON.stringify(stream)}`);
  return stream;
};

type Query = (
  sql: string,
  ...params: unknown[]
) => Promise<Record<string, unknown>[]>;

/** `items` in groups of at most `SQL.STREAM_BATCH`, in order. */
const batches = <T,>(items: readonly T[]): T[][] => {
  const groups: T[][] = [];
  for (let at = 0; at < items.length; at += SQL.STREAM_BATCH)
    groups.push(items.slice(at, at + SQL.STREAM_BATCH));
  return groups;
};

/** Canonical Stream order: UTF-8 byte order, which is code point order. */
const byteOrder = (a: string, b: string): number => {
  const x = [...a].map((c) => c.codePointAt(0)!);
  const y = [...b].map((c) => c.codePointAt(0)!);
  for (let i = 0; i < Math.min(x.length, y.length); i++)
    if (x[i] !== y[i]) return x[i]! - y[i]!;
  return x.length - y.length;
};

/** A request object naming exactly `fields`; another field is refused before any SQL runs. */
const fieldsOf = (
  value: unknown,
  fields: readonly string[],
  what: string,
): Record<string, unknown> => {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error(`${what} must be an object`);
  for (const field of Object.keys(value))
    if (!fields.includes(field))
      throw new Error(`Unknown ${what} field ${field}`);
  return value as Record<string, unknown>;
};
const nonEmpty = (value: unknown, what: string): string => {
  if (typeof value !== "string" || value === "")
    throw new Error(`${what} must be a non-empty string`);
  return value;
};
const strings = (value: unknown, what: string): string[] => {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string"))
    throw new Error(`${what} must be an array of strings`);
  if (new Set(value).size !== value.length)
    throw new Error(`${what} repeats an entry`);
  return value as string[];
};
/** A JSON column: drivers hand back parsed values; a string is parsed. */
const json = (value: unknown): unknown =>
  typeof value === "string" ? JSON.parse(value) : value;

const keyOf = (
  value: unknown,
  extra: readonly string[] = [],
): Record<string, unknown> & { model: string; identityKey: string } => {
  const key = fieldsOf(value, ["model", "identityKey", ...extra], "record key");
  return {
    ...key,
    model: nonEmpty(key.model, "record model"),
    identityKey: nonEmpty(key.identityKey, "record identityKey"),
  };
};
const pairId = (p: { model: string; identityKey: string; stream?: string }) =>
  JSON.stringify([p.stream, p.model, p.identityKey]);
const arrayOf = (value: unknown, what: string): unknown[] => {
  if (!Array.isArray(value)) throw new Error(`${what} must be an array`);
  return value;
};
const unique = (
  values: { model: string; identityKey: string; stream?: string }[],
  what: string,
) => {
  if (new Set(values.map(pairId)).size !== values.length)
    throw new Error(`${what} repeats a key`);
};
async function lockStreams(q: Query, r: unknown): Promise<Acknowledged> {
  const request = fieldsOf(r, ["op", "streams"], "lockStreams");
  const streams = strings(request.streams, "lockStreams streams");
  if (streams.length === 0)
    throw new Error("lockStreams needs at least one Stream");
  streams.forEach(streamName);
  for (let i = 1; i < streams.length; i++)
    if (byteOrder(streams[i - 1]!, streams[i]!) >= 0)
      throw new Error("lockStreams needs canonical order");
  for (const group of batches(streams))
    await q(SQL.LOCK_STREAMS, JSON.stringify(group));
  return null;
}
async function answer05(
  q: Query,
  r: Record<string, unknown>,
): Promise<unknown> {
  switch (r.op) {
    case "bootstrapState": {
      const [row] = await q(
        "SELECT bootstrap_prepared FROM axton_store WHERE id=$1",
        r.storeId,
      );
      if (!row) throw new Error("Store missing");
      return row.bootstrap_prepared;
    }
    case "finishBootstrap": {
      await q(
        "UPDATE axton_store SET bootstrap_prepared=true,start_cursor=(SELECT COALESCE((SELECT head FROM axton_stream WHERE stream=axton_store.stream),0)) WHERE id=$1",
        r.storeId,
      );
      return null;
    }
    case "deliveryHead": {
      const [row] = await q(
        "SELECT head FROM axton_stream WHERE stream=$1",
        r.stream,
      );
      return row ? safe(row.head) : 0;
    }
    case "deliveryNow": {
      const [row] = await q(
        "SELECT floor(extract(epoch from clock_timestamp())*1000)::bigint now",
      );
      return safe(row!.now);
    }
    case "deliveryCandidates": {
      const models = r.models as string[] | null,
        keys = r.keys as MemberKey[] | null;
      const cap = Math.min(safe(r.capacity), 100000);
      if (keys?.length) {
        for (const group of batches(keys)) {
          const tracked = await q(
            "SELECT count(*) n FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id JOIN jsonb_array_elements($2::jsonb) k ON k->>'model'=r.model AND k->>'identityKey'=r.identity_key WHERE s.stream=$1",
            r.stream,
            JSON.stringify(group),
          );
          if (safe(tracked[0]!.n) !== group.length)
            throw new Error("delivery.identity_untracked");
        }
      }
      const pageSize =
        r.pageSize == null ? null : Math.min(positiveCounter(r.pageSize), cap);
      const select = `SELECT r.model,r.identity_key,s.cursor,s.kind FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id WHERE s.stream=$1 AND s.cursor>$2 AND ($3::text[] IS NULL OR r.model=ANY($3::text[]) OR EXISTS(SELECT 1 FROM jsonb_array_elements($4::jsonb) k WHERE k->>'model'=r.model AND k->>'identityKey'=r.identity_key))`;
      const rows = await q(
        pageSize === null
          ? `${select} ORDER BY s.cursor,r.model COLLATE "C",r.identity_key COLLATE "C" LIMIT $5`
          : `SELECT * FROM (${select} ORDER BY s.cursor FETCH FIRST $6 ROWS WITH TIES) page LIMIT $5`,
        r.stream,
        safe(r.after),
        models,
        JSON.stringify(keys ?? []),
        cap + 1,
        ...(pageSize === null ? [] : [pageSize + 1]),
      );
      if (rows.length > cap) return null;
      return rows.map((row) => ({
        model: row.model,
        identityKey: row.identity_key,
        cursor: positiveCounter(row.cursor),
        kind: row.kind,
      }));
    }
    case "handleBootstrap05":
      throw new Error("bootstrap belongs to application host");
    case "inspectStore": {
      const [row] = await q(SQL.V05_STORE_INSPECT, r.storeId);
      return row ? { principal: row.principal, stream: row.stream } : null;
    }
    case "claimStore": {
      let [row] = await q(SQL.V05_STORE_LOCK, r.storeId);
      if (!row) {
        // First claims still race through the unique Store identity. A stale
        // Serializable snapshot retries the whole transaction as before.
        await q(SQL.V05_STORE_INSERT, r.storeId, r.principal, r.stream);
        [row] = await q(SQL.V05_STORE_LOCK, r.storeId);
      }
      if (!row) throw new Error("Store missing");
      return {
        principal: row.principal,
        stream: row.stream,
        lastProcessedBatchId: safe(row.last_processed_batch_id),
        progress: safe(row.progress),
        currentDigest: row.current_digest,
        currentCount:
          row.current_count === null ? null : safe(row.current_count),
        lastDigest: row.last_digest,
        lastCount: row.last_count === null ? null : safe(row.last_count),
      };
    }
    case "beginBatch": {
      const rows = await q(
        SQL.V05_BATCH_BEGIN,
        r.storeId,
        r.batchId,
        r.digest,
        r.count,
      );
      if (rows.length !== 1) throw new Error("Batch admission state changed");
      return null;
    }
    case "readResult": {
      const [row] = await q(
        SQL.V05_RESULT_READ,
        r.storeId,
        r.batchId,
        r.ordinal,
      );
      return row ? json(row.result) : null;
    }
    case "readResults":
      return (await q(SQL.V05_RESULTS_READ, r.storeId, r.batchId)).map((row) =>
        json(row.result),
      );
    case "saveResult": {
      const result = r.result as { mutationId: number };
      const rows = await q(
        SQL.V05_RESULT_SAVE,
        r.storeId,
        r.batchId,
        r.ordinal,
        r.count,
        result.mutationId,
        JSON.stringify(result),
      );
      if (rows.length !== 1) throw new Error("Batch progress state changed");
      return null;
    }
    case "saveResults": {
      const results = arrayOf(r.results, "group results");
      if (results.length === 0 || results.length > 32)
        throw new Error("Invalid result group size");
      const ordinal = safe(r.ordinal),
        count = positiveCounter(r.count);
      if (ordinal + results.length > count)
        throw new Error("Invalid result group range");
      const rows = await q(
        SQL.V05_RESULTS_SAVE,
        r.storeId,
        r.batchId,
        ordinal,
        count,
        results.length,
        r.digest,
        JSON.stringify(results),
      );
      if (rows.length !== results.length)
        throw new Error("Batch progress state changed");
      return null;
    }
    case "readTracking": {
      const rows = await q(
        SQL.READ_TRACKING,
        JSON.stringify(r.records),
        JSON.stringify(r.pairs),
      );
      return rows.map((row) => ({
        stream: row.stream,
        model: row.model,
        identityKey: row.identity_key,
      }));
    }
    case "guardRecords": {
      const records = arrayOf(r.records, "guards").map((value) =>
        keyOf(value, ["mode"]),
      );
      unique(records, "guards");
      for (let i = 0; i < records.length; i++) {
        const record = records[i];
        if (!["ensure", "lock"].includes(String(record.mode)))
          throw new Error("invalid guard mode");
        if (i > 0) {
          const prior = records[i - 1];
          if (
            byteOrder(prior.model, record.model) > 0 ||
            (prior.model === record.model &&
              byteOrder(prior.identityKey, record.identityKey) >= 0)
          )
            throw new Error("guards must be canonically ordered");
        }
      }
      const result: boolean[] = [];
      // Never reorder mixed modes: an earlier lock must precede a later ensure.
      for (let at = 0; at < records.length;) {
        const mode = records[at]!.mode;
        let end = at + 1;
        while (
          end < records.length &&
          end - at < SQL.STREAM_BATCH &&
          records[end]!.mode === mode
        )
          end++;
        const group = records.slice(at, end);
        const rows = await q(
          mode === "lock" ? SQL.LOCK_IDENTITIES : SQL.ENSURE_IDENTITIES,
          JSON.stringify(group),
        );
        const represented = new Set(
          rows.map((row) =>
            pairId({
              model: String(row.model),
              identityKey: String(row.identity_key),
            }),
          ),
        );
        result.push(...group.map((record) => represented.has(pairId(record))));
        at = end;
      }
      return result;
    }
    case "targetPositions":
    case "readPositions": {
      const records = arrayOf(r.records, "positions").map((value) =>
        keyOf(value),
      );
      const positions = [];
      for (const group of batches(records)) {
        const rows = await q(
          SQL.V05_POSITIONS_READ_BATCH,
          r.stream,
          JSON.stringify(group),
        );
        if (rows.length !== group.length)
          throw new Error("position cardinality mismatch");
        for (let i = 0; i < group.length; i++) {
          const record = group[i]!;
          const row = rows[i]!;
          if (row.cursor === null) {
            if (r.op === "readPositions")
              throw new Error("missing Stream position");
            positions.push(null);
          } else
            positions.push({
              stream: r.stream,
              model: record.model,
              identityKey: record.identityKey,
              cursor: positiveCounter(row.cursor),
              kind: row.kind,
            });
        }
      }
      return positions;
    }
    case "applyStreamMembers": {
      const deltas = arrayOf(r.deltas, "deltas").map((value) =>
        keyOf(value, ["identity", "stream", "publish"]),
      );
      unique(
        deltas as { model: string; identityKey: string; stream?: string }[],
        "deltas",
      );
      const streams = [
        ...new Set(
          deltas.filter((d) => d.publish).map((d) => streamName(d.stream)),
        ),
      ].sort(byteOrder);
      // SQL owns reservation lifetime: both rows and their first table creation
      // roll back with caller savepoints, and COMMIT clears connection-local rows.
      // Repeated preparation/publication in this transaction reuses one cursor.
      if (streams.length) await q(SQL.V05_PUBLICATION_CURSORS);
      const cursors = new Map<string, number>();
      for (const stream of streams) {
        const [row] = await q(
          SQL.V05_RESERVE_CURSOR,
          stream,
          r.publicationScope ?? "",
        );
        if (!row) throw new Error(`Stream ${stream} head counter overflow`);
        cursors.set(stream, positiveCounter(row.cursor));
      }
      const positions = [];
      for (const group of batches(deltas)) {
        const rows = await q(
          SQL.V05_APPLY_MEMBERS_BATCH,
          JSON.stringify(
            group.map((d) => ({
              ...d,
              stream: streamName(d.stream),
              cursor: cursors.get(String(d.stream)),
            })),
          ),
        );
        if (rows.length !== group.length)
          throw new Error("member cardinality mismatch");
        for (let i = 0; i < group.length; i++) {
          const d = group[i]!;
          const row = rows[i]!;
          if (!row.represented)
            throw new Error("missing canonical record metadata");
          if (row.kind !== "upsert")
            throw new Error("unpublished pair has no live position");
          positions.push({
            stream: d.stream,
            model: d.model,
            identityKey: d.identityKey,
            cursor: positiveCounter(row.cursor),
            kind: "upsert",
          });
        }
      }
      return positions;
    }
    default:
      throw new Error(
        `unsupported protocol05 persistence operation ${String(r.op)}`,
      );
  }
}

/**
 * Answer the persistence half of the host contract through a driver, inside
 * the current application transaction. `handleAction` and `load` never reach
 * here; an operation added to the contract without an arm is a compile error.
 * Standalone callers have no initialized adapter, so this low-level entry
 * point validates their namespace before executing its one operation.
 */
export async function answer<Tx>(
  driver: PostgresDriver<Tx>,
  tx: Tx,
  r: HostRequest,
): Promise<unknown> {
  await requireFreshLayout((sql, ...params) => driver.query(tx, sql, params));
  return answerInitialized(driver, tx, r);
}

/** Dispatch after this adapter's startup check; never re-scan the catalog. */
async function answerInitialized<Tx>(
  driver: PostgresDriver<Tx>,
  tx: Tx,
  r: HostRequest,
): Promise<unknown> {
  const q = (sql: string, ...params: unknown[]) =>
    driver.query(tx, sql, params);
  switch (r.op) {
    case "protocol05":
      return answer05(q, r.request);
    case "head": {
      const [row] = await q(SQL.HEAD, r.stream);
      return row ? safe(row.head) : 0;
    }
    case "lockStreams":
      return lockStreams(q, r);
    case "readTracking":
    case "guardRecords":
    case "applyStreamMembers":
      return answer05(q, r);
    case "savepoint":
    case "rollback":
    case "release": {
      if (!Number.isSafeInteger(r.ordinal) || r.ordinal < 1)
        throw new Error("Invalid savepoint ordinal");
      const name = SQL.savepointName(r.ordinal);
      const command =
        r.op === "savepoint"
          ? "SAVEPOINT"
          : r.op === "rollback"
            ? "ROLLBACK TO SAVEPOINT"
            : "RELEASE SAVEPOINT";
      await q(`${command} ${name}`);
      const acknowledged: Acknowledged = null;
      return acknowledged;
    }
    case "handleAction":
    case "load":
      break;
    default: {
      const unreachable: never = r;
      void unreachable;
    }
  }
  throw new Error(
    `Unsupported persistence operation ${(r as { op: string }).op}`,
  );
}

/**
 * The `database` option of `createBackend`, built on any driver: the driver's
 * transaction runner plus a persistence bound to each transaction.
 */
export function persistence<Tx>(driver: PostgresDriver<Tx>): Database<Tx> & {
  driver: PostgresDriver<Tx>;
  initialize: (tx?: Tx) => Promise<void>;
} {
  let initializing: Promise<void> | undefined;
  const initialize = (tx?: Tx): Promise<void> => {
    const check = (retained: Tx) =>
      requireFreshLayout((sql, ...params) =>
        driver.query(retained, sql, params),
      );
    return (initializing ??= Promise.resolve()
      .then(() => (tx === undefined ? driver.transaction(check) : check(tx)))
      .catch((error: unknown) => {
        initializing = undefined;
        throw error;
      }));
  };
  return {
    driver,
    mutationCommitGroups: true,
    mutationResultGroups: true,
    initialize,
    transaction: (body, diagnostics) =>
      driver.transaction(
        (tx) =>
          diagnostics
            ? diagnostics.span("postgres.transaction.body", {}, () => body(tx))
            : body(tx),
        diagnostics,
      ),
    persistence: (tx: Tx, diagnostics): Persistence => ({
      call: async (request) => {
        await initialize(tx);
        const traced = diagnostics
          ? {
              ...driver,
              query: (retained: Tx, sql: string, params: readonly unknown[]) =>
                diagnostics.span(
                  "postgres.query",
                  { command: sql.trimStart().split(/\s/, 1)[0] },
                  () => driver.query(retained, sql, params),
                ),
            }
          : driver;
        return answerInitialized(traced, tx, request as HostRequest);
      },
    }),
  };
}

async function requireFreshLayout(q: Query): Promise<void> {
  const [row] = await q(SQL.CHECK_LAYOUT);
  if (!row || row.legacy !== false)
    throw new Error(
      "installed legacy framework layout: protocol 5 requires a fresh namespace; existing data is unchanged",
    );
}
