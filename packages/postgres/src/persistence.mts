import type {
  Acknowledged,
  Claimed,
  ClaimedCall,
  Head,
  HostRequest,
  Invalidation,
  Published,
  Stamped,
} from "../../server/host-contract.mts";
import type { Database, Persistence } from "../../server/index.mts";
import type { PostgresDriver } from "./driver.mts";
import * as SQL from "./sql.mts";

const safe = (n: unknown): number => {
  const number = Number(n);
  if (!Number.isSafeInteger(number) || number < 0)
    throw new Error("Stored counter outside safe range");
  return number;
};

/**
 * Answer the persistence half of the host contract through a driver, inside
 * the transaction the driver's runner opened. `handle` and `load` never reach
 * here; an operation added to the contract without an arm is a compile error.
 */
export async function answer<Tx>(
  driver: PostgresDriver<Tx>,
  tx: Tx,
  r: HostRequest,
): Promise<unknown> {
  const q = (sql: string, ...params: unknown[]) =>
    driver.query(tx, sql, params);
  switch (r.op) {
    case "claim": {
      await q(SQL.CLAIM_INSERT, r.clientId, r.owner);
      const rows = await q(SQL.CLAIM_LOCK, r.clientId);
      if (rows.length !== 1) throw new Error("Failed to lock client");
      const row = rows[0]!;
      const claimed: Claimed = {
        clientId: String(row.client_id),
        owner: String(row.owner_id),
        sequence: safe(row.sequence),
        receipt: row.receipt === null ? null : String(row.receipt),
      };
      return claimed;
    }
    case "saveReceipt": {
      const rows = await q(
        SQL.SAVE_RECEIPT,
        r.clientId,
        r.owner,
        BigInt(r.sequence),
        r.receipt,
      );
      if (rows.length !== 1) throw new Error("Receipt owner mismatch");
      const acknowledged: Acknowledged = null;
      return acknowledged;
    }
    case "claimCall": {
      const inserted = await q(SQL.CLAIM_CALL_INSERT, r.owner, r.callId, r.request);
      const rows = await q(SQL.CLAIM_CALL_LOCK, r.owner, r.callId);
      if (rows.length !== 1) throw new Error("Failed to lock call");
      const row = rows[0]!;
      if (inserted.length === 0 && row.response === null)
        throw new Error("Call has incomplete stored response");
      const claimed: ClaimedCall = {
        fresh: inserted.length === 1,
        request: String(row.request),
        response: row.response === null ? null : String(row.response),
      };
      return claimed;
    }
    case "saveCall": {
      const rows = await q(SQL.SAVE_CALL, r.owner, r.callId, r.response);
      if (rows.length !== 1) throw new Error("Call not claimed or already completed");
      const acknowledged: Acknowledged = null;
      return acknowledged;
    }
    case "head": {
      const rows = await q(SQL.HEAD, r.channel);
      const head: Head = rows.length ? safe(rows[0]!.head) : 0;
      return head;
    }
    case "scan": {
      const rows = await q(SQL.SCAN, r.channel, BigInt(r.after), r.limit);
      const scanned: Invalidation[] = rows.map((row) => {
        if (row.stamp === null || row.stamp === undefined)
          throw new Error(
            `Record metadata missing for ${row.model} ${row.identity_key} on channel ${row.channel}`,
          );
        return {
          channel: String(row.channel),
          cursor: safe(row.cursor),
          model: String(row.model),
          identityKey: String(row.identity_key),
          identity: row.identity as Record<string, unknown>,
          stamp: safe(row.stamp),
        };
      });
      return scanned;
    }
    case "advanceStamp": {
      const rows = await q(SQL.ADVANCE_STAMP, r.model, r.identityKey);
      const stamped: Stamped = safe(rows[0]!.stamp);
      return stamped;
    }
    case "ensureStamp": {
      const rows = await q(SQL.ENSURE_STAMP, r.model, r.identityKey);
      const stamped: Stamped = safe(rows[0]!.stamp);
      return stamped;
    }
    case "publish": {
      // Distribution allocates only the channel cursor. The record row is
      // locked and must carry the stamp the request names: a stale one is a
      // defect of the caller's ordering, never silently re-stamped.
      const locked = await q(SQL.LOCK_STAMP, r.model, r.identityKey);
      if (locked.length !== 1)
        throw new Error(
          `Record metadata missing for ${r.model} ${r.identityKey}: publish needs its stamp first`,
        );
      const stamp = safe(locked[0]!.stamp);
      if (stamp !== r.stamp)
        throw new Error(
          `Publication names stamp ${r.stamp} but ${r.model} ${r.identityKey} is at stamp ${stamp}`,
        );
      const rows = await q(SQL.ADVANCE_HEAD, r.channel);
      const cursor = safe(rows[0]!.head);
      await q(
        SQL.UPSERT_INVALIDATION,
        r.channel,
        r.model,
        r.identityKey,
        JSON.stringify(r.identity),
        BigInt(cursor),
        BigInt(stamp),
      );
      const published: Published = { cursor, stamp };
      return published;
    }
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
    case "handle":
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
export function persistence<Tx>(
  driver: PostgresDriver<Tx>,
): Database<Tx> & { driver: PostgresDriver<Tx> } {
  return {
    driver,
    transaction: (body) => driver.transaction(body),
    persistence: (tx: Tx): Persistence => ({
      call: (request) => answer(driver, tx, request as HostRequest),
    }),
  };
}
