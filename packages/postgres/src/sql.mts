/** Every statement AXTON runs against PostgreSQL. Tables: `migration.sql`. */
export const CLAIM_INSERT =
  "INSERT INTO axton_client (client_id, owner_id) VALUES ($1,$2) ON CONFLICT (client_id) DO NOTHING";
export const CLAIM_LOCK =
  "SELECT client_id, owner_id, sequence, receipt FROM axton_client WHERE client_id=$1 FOR UPDATE";
export const SAVE_RECEIPT =
  "UPDATE axton_client SET sequence=$3, receipt=$4 WHERE client_id=$1 AND owner_id=$2 RETURNING client_id";
export const HEAD = "SELECT head FROM axton_channel WHERE channel=$1";
/**
 * The invalidation keeps its own cursor (delivery progress); the stamp is the
 * record's current one, read in the same snapshot the loader reads. A record
 * with no metadata is a storage defect: the join is outer so it is reported,
 * never dropped as a missing row.
 */
export const SCAN =
  "SELECT i.channel, i.cursor, i.model, i.identity_key, i.identity, r.stamp FROM axton_invalidation i LEFT JOIN axton_record r ON r.model=i.model AND r.identity_key=i.identity_key WHERE i.channel=$1 AND i.cursor>$2 ORDER BY i.cursor LIMIT $3";
/** The upsert locks the record row, so concurrent changes never share a stamp. */
export const ADVANCE_STAMP =
  "INSERT INTO axton_record(model,identity_key,stamp) VALUES($1,$2,1) ON CONFLICT(model,identity_key) DO UPDATE SET stamp=axton_record.stamp+1 RETURNING stamp";
/**
 * Initialise at 1 only when the record has no stamp. The no-op update (rather
 * than DO NOTHING plus a SELECT) makes a row another transaction initialised
 * after our snapshot surface as a serialization failure the runner retries.
 */
export const ENSURE_STAMP =
  "INSERT INTO axton_record(model,identity_key,stamp) VALUES($1,$2,1) ON CONFLICT(model,identity_key) DO UPDATE SET stamp=axton_record.stamp RETURNING stamp";
export const LOCK_STAMP =
  "SELECT stamp FROM axton_record WHERE model=$1 AND identity_key=$2 FOR UPDATE";
export const ADVANCE_HEAD =
  "INSERT INTO axton_channel(channel,head) VALUES($1,1) ON CONFLICT(channel) DO UPDATE SET head=axton_channel.head+1 RETURNING head";
export const UPSERT_INVALIDATION =
  "INSERT INTO axton_invalidation(channel,model,identity_key,identity,cursor,stamp) VALUES($1,$2,$3,$4::jsonb,$5,$6) ON CONFLICT(channel,model,identity_key) DO UPDATE SET identity=EXCLUDED.identity,cursor=EXCLUDED.cursor,stamp=EXCLUDED.stamp";
export const savepointName = (ordinal: number): string =>
  `axton_mutation_${ordinal}`;
