//! The server's persistence, in memory. Mirrors packages/postgres/src/persistence.mts
//! closely enough that axton_server cannot tell the difference: per-client receipts,
//! per-channel heads, one invalidation row per (channel, record) carrying the latest
//! cursor, and one stamp counter per record that only a business change advances.
use axton_core::{PushRequest, RecordKey};
use axton_server::{
    Host,
    host::{
        Acknowledged, Claimed, ClaimedCall, Handled, Head, HostRequest,
        Invalidation as ContractInvalidation, Loaded, PublicationIntent, Published, RecordRef,
        Scanned, Stamped,
    },
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

#[derive(Clone)]
struct Invalidation {
    model: String,
    identity: Value,
    identity_key: String,
    cursor: u64,
    /// The stamp this channel was last told about. `scan` answers with the record's
    /// *current* stamp instead (see `Tables::stamps`); this one only says how far
    /// behind the record this channel's own invalidation is.
    stamp: u64,
}

/// One record's stamp row: the counter, and the evidence the invariant
/// `republication_cannot_advance_a_stamp` compares it against.
#[derive(Clone)]
struct Stamp {
    key: RecordKey,
    value: u64,
    /// Committed `advanceStamp` calls. A rolled-back mutation's advance rolls back
    /// with the tables, exactly as the counter itself does.
    advances: u64,
    /// Whether `ensureStamp` initialized this record (first publication of a record
    /// that had no stamp metadata). Never set once `advanceStamp` has run.
    initialized: bool,
}

#[derive(Clone, Default)]
struct Tables {
    calls: BTreeMap<(String, String), (String, Option<String>)>,
    records: BTreeMap<String, Value>,
    stamps: BTreeMap<String, Stamp>,
    heads: BTreeMap<String, u64>,
    invalidations: BTreeMap<(String, String), Invalidation>,
}

#[derive(Default)]
struct State {
    tables: Tables,
    membership: BTreeMap<String, Vec<String>>,
    clients: BTreeMap<String, Claimed>,
    savepoints: Vec<Tables>,
    reject_next: Option<String>,
    fail_next: bool,
    break_next: bool,
    uppercase_next: bool,
    /// Records whose next single-identity `load` throws (a batched load naming
    /// one of them throws too, which is what makes the engine retry per identity).
    fail_load: BTreeSet<String>,
    /// Records whose next single-identity `load` is refused with `sim.refused`.
    refuse_load: BTreeSet<String>,
    handler_calls: usize,
    accepted: usize,
    rejected: usize,
    failed: usize,
    /// The (clientId, batchSequence) of the push currently being processed, decoded
    /// from the raw request bytes in `push()` - the `handle` op itself carries only
    /// `ordinal` (see `crates/server/src/lib.rs::process_push`), and the server's wire
    /// contract with its host is not otherwise touched by this bookkeeping.
    current_push: Option<(String, u64)>,
    /// (clientId, batchSequence, ordinal) for every `handle` call, in call order.
    /// `no_mutation_executes_twice` asserts these are pairwise distinct: a retry that
    /// reached the handler again (rather than being short-circuited by the stored
    /// receipt) would duplicate one.
    handler_invocations: Vec<(String, u64, u64)>,
}

pub struct MemHost(Mutex<State>);

pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut f = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(result) = f.as_mut().poll(&mut cx) {
            return result;
        }
    }
}

fn encoded(key: &RecordKey) -> String {
    key.encoded().unwrap()
}

/// The record a stamp request names: its `identity_key` is the canonical identity.
fn key_from_identity_key(model: &str, identity_key: &str) -> Result<RecordKey, String> {
    let identity: Value =
        serde_json::from_str(identity_key).map_err(|e| format!("bad identity key: {e}"))?;
    crate::schema::schema()
        .record_key(model, &identity)
        .map_err(|e| e.to_string())
}

impl Default for MemHost {
    fn default() -> Self {
        Self::new()
    }
}

impl MemHost {
    pub fn new() -> Self {
        Self(Mutex::new(State::default()))
    }
    pub fn set_membership(&self, key: &RecordKey, channels: &[&str]) {
        self.0.lock().unwrap().membership.insert(
            encoded(key),
            channels.iter().map(|c| c.to_string()).collect(),
        );
    }
    pub fn membership(&self, key: &RecordKey) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .membership
            .get(&encoded(key))
            .cloned()
            .unwrap_or_default()
    }
    /// Whether membership was ever explicitly set for `key`, even to no channels at
    /// all - distinct from `membership` being empty because nothing was set yet.
    pub fn has_membership(&self, key: &RecordKey) -> bool {
        self.0
            .lock()
            .unwrap()
            .membership
            .contains_key(&encoded(key))
    }
    /// One business change to `key`, distributed to `channels`: the stamp advances
    /// once and every channel is invalidated at that same stamp. This is what
    /// `axton_server::publish` does for an external notification.
    pub fn notify(&self, key: &RecordKey, channels: &[&str]) {
        let mut s = self.0.lock().unwrap();
        let stamp = advance(&mut s.tables, key);
        for c in channels {
            publish_at(&mut s.tables, c, key, stamp).expect("current stamp");
        }
    }
    /// Republish `key` on `channel` at its current stamp, initializing the stamp at 1
    /// only if the record has none: a channel learning of an existing record, which
    /// never advances its version.
    pub fn ensure_publish(&self, key: &RecordKey, channel: &str) {
        let mut s = self.0.lock().unwrap();
        let stamp = ensure(&mut s.tables, key);
        publish_at(&mut s.tables, channel, key, stamp).expect("current stamp");
    }
    pub fn set_state(&self, key: &RecordKey, state: Option<Value>) {
        let mut s = self.0.lock().unwrap();
        match state {
            Some(v) => s.tables.records.insert(encoded(key), v),
            None => s.tables.records.remove(&encoded(key)),
        };
    }
    pub fn reject_next(&self, code: &str) {
        self.0.lock().unwrap().reject_next = Some(code.into());
    }
    /// The next `handle` call is answered `Handled::Failed`: the engine rejects
    /// just that mutation with `handler.failed` and the rest of the batch stands.
    pub fn fail_next(&self) {
        self.0.lock().unwrap().fail_next = true;
    }
    /// The next `rollback` (which follows a rejected or failed mutation) answers
    /// with a host error instead: an infrastructure failure, not a business
    /// rejection, so the whole delivery fails and nothing in it is committed.
    pub fn break_next(&self) {
        self.0.lock().unwrap().break_next = true;
    }
    /// The next accepted mutation stores its `text` uppercased: the server
    /// "normalizing" a value, so the receipt's authority differs from the optimism.
    pub fn uppercase_next(&self) {
        self.0.lock().unwrap().uppercase_next = true;
    }
    /// The loader throws for `key` until it is asked for `key` alone: the
    /// batched load fails, the engine retries per identity, and only this
    /// record comes back as an error change.
    pub fn fail_load_next(&self, key: &RecordKey) {
        self.0.lock().unwrap().fail_load.insert(encoded(key));
    }
    /// Like [`fail_load_next`](Self::fail_load_next) with a refusal (`sim.refused`).
    pub fn refuse_load_next(&self, key: &RecordKey) {
        self.0.lock().unwrap().refuse_load.insert(encoded(key));
    }
    pub fn state(&self, key: &RecordKey) -> Option<Value> {
        self.0
            .lock()
            .unwrap()
            .tables
            .records
            .get(&encoded(key))
            .cloned()
    }
    pub fn records(&self) -> BTreeMap<String, Value> {
        self.0.lock().unwrap().tables.records.clone()
    }
    /// The record's current stamp: its content version, shared by every channel.
    pub fn stamp(&self, key: &RecordKey) -> u64 {
        self.0
            .lock()
            .unwrap()
            .tables
            .stamps
            .get(&encoded(key))
            .map(|s| s.value)
            .unwrap_or(0)
    }
    /// Per stamped record: (key, stamp, committed advances, initialized by
    /// `ensureStamp`). See `republication_cannot_advance_a_stamp` in invariants.rs.
    pub fn stamp_accounting(&self) -> Vec<(RecordKey, u64, u64, bool)> {
        self.0
            .lock()
            .unwrap()
            .tables
            .stamps
            .values()
            .map(|s| (s.key.clone(), s.value, s.advances, s.initialized))
            .collect()
    }
    pub fn head(&self, channel: &str) -> u64 {
        self.0
            .lock()
            .unwrap()
            .tables
            .heads
            .get(channel)
            .copied()
            .unwrap_or(0)
    }
    pub fn handler_calls(&self) -> usize {
        self.0.lock().unwrap().handler_calls
    }
    /// (clientId, batchSequence, ordinal) for every `handle` call the host has made,
    /// in call order. See `no_mutation_executes_twice` in invariants.rs.
    pub fn handler_invocations(&self) -> Vec<(String, u64, u64)> {
        self.0.lock().unwrap().handler_invocations.clone()
    }
    pub fn accepted(&self) -> usize {
        self.0.lock().unwrap().accepted
    }
    pub fn rejected(&self) -> usize {
        self.0.lock().unwrap().rejected
    }
    pub fn failed(&self) -> usize {
        self.0.lock().unwrap().failed
    }
    /// Every record that holds a stamp, published or not.
    pub fn stamped_keys(&self) -> Vec<RecordKey> {
        self.0
            .lock()
            .unwrap()
            .tables
            .stamps
            .values()
            .map(|s| s.key.clone())
            .collect()
    }
    /// The stamp `channel` was last invalidated for `key` at, if ever. A change
    /// published to a subset of a record's channels leaves the others behind
    /// `stamp(key)` until they are next told.
    pub fn channel_stamp(&self, channel: &str, key: &RecordKey) -> Option<u64> {
        self.0
            .lock()
            .unwrap()
            .tables
            .invalidations
            .get(&(channel.to_string(), encoded(key)))
            .map(|row| row.stamp)
    }
    pub fn channel_records(&self, channel: &str) -> Vec<RecordKey> {
        let s = self.0.lock().unwrap();
        s.tables
            .invalidations
            .iter()
            .filter(|((c, _), _)| c == channel)
            .map(|(_, row)| key_of(&row.model, &row.identity))
            .collect()
    }
    pub fn receipt(&self, client_id: &str, sequence: u64) -> Option<String> {
        let s = self.0.lock().unwrap();
        let row = s.clients.get(client_id)?;
        if row.sequence == sequence {
            row.receipt.clone()
        } else {
            None
        }
    }
    /// The last batch sequence the server accepted from `client_id` (0 if none).
    pub fn client_sequence(&self, client_id: &str) -> u64 {
        self.0
            .lock()
            .unwrap()
            .clients
            .get(client_id)
            .map(|c| c.sequence)
            .unwrap_or(0)
    }
    pub fn push(&self, owner: &str, bytes: &[u8]) -> Result<String, String> {
        // The `handle` op carries only `ordinal`, not `clientId` or `batchSequence`
        // (see `process_push` in crates/server) - decode the raw request here, at the
        // one place that already has the bytes, rather than growing the wire protocol
        // between the server and its host just for this bookkeeping.
        if let Ok(request) = PushRequest::decode(bytes) {
            self.0.lock().unwrap().current_push = Some((request.client_id, request.batch_sequence));
        }
        let (before, depth, invocations) = {
            let s = self.0.lock().unwrap();
            (
                s.tables.clone(),
                s.savepoints.len(),
                s.handler_invocations.len(),
            )
        };
        let result = block_on(axton_server::process_push(
            &crate::schema::config(),
            owner,
            bytes,
            self,
        ))
        .map_err(|e| e.to_string());
        if result.is_err() {
            // An aborted batch is a rolled-back transaction: nothing it did survives.
            // A `handle` error short-circuits process_push after `savepoint` but before
            // the matching `release`, so the savepoint stack must also be restored to
            // its pre-call depth here — this is the same invariant a successful push
            // already leaves it at (every `savepoint` is paired with a `release`).
            //
            // The client legitimately retries the same bytes after this (P6), and that
            // retry will call `handle` again for the same ordinals - that is correct,
            // not a double execution, because nothing from this attempt was ever
            // durably accepted. So `handler_invocations` rolls back with the tables:
            // only a triple recorded by a push that actually committed counts toward
            // `no_mutation_executes_twice`.
            let mut s = self.0.lock().unwrap();
            s.tables = before;
            s.savepoints.truncate(depth);
            s.handler_invocations.truncate(invocations);
        }
        result
    }
    pub fn savepoint_depth(&self) -> usize {
        self.0.lock().unwrap().savepoints.len()
    }
    pub fn pull(&self, owner: &str, bytes: &[u8]) -> Result<String, String> {
        block_on(axton_server::process_pull(
            &crate::schema::config(),
            owner,
            bytes,
            self,
        ))
        .map_err(|e| e.to_string())
    }
}

/// `advanceStamp`: initialize at 1 or increment, counting the advance.
fn advance(t: &mut Tables, key: &RecordKey) -> u64 {
    let row = t.stamps.entry(encoded(key)).or_insert_with(|| Stamp {
        key: key.clone(),
        value: 0,
        advances: 0,
        initialized: false,
    });
    row.value += 1;
    row.advances += 1;
    row.value
}

/// `ensureStamp`: the current stamp, initialized at 1 only when there is none.
fn ensure(t: &mut Tables, key: &RecordKey) -> u64 {
    t.stamps
        .entry(encoded(key))
        .or_insert_with(|| Stamp {
            key: key.clone(),
            value: 1,
            advances: 0,
            initialized: true,
        })
        .value
}

/// `publish`: invalidate `key` on `channel` at `stamp`, which must be the record's
/// current stamp. Allocates only the channel's cursor.
fn publish_at(t: &mut Tables, channel: &str, key: &RecordKey, stamp: u64) -> Result<u64, String> {
    let k = encoded(key);
    let current = t.stamps.get(&k).map(|s| s.value);
    if current != Some(stamp) {
        return Err(format!(
            "publish of {k} at stamp {stamp}, record is at {current:?}"
        ));
    }
    let head = t.heads.entry(channel.to_string()).or_insert(0);
    *head += 1;
    let cursor = *head;
    t.invalidations.insert(
        (channel.to_string(), k),
        Invalidation {
            model: key.model.clone(),
            identity: key.identity.clone(),
            identity_key: key.encoded_identity().unwrap(),
            cursor,
            stamp,
        },
    );
    Ok(cursor)
}

fn key_of(model: &str, identity: &Value) -> RecordKey {
    crate::schema::schema().record_key(model, identity).unwrap()
}

/// Apply one decoded handler argument to the business tables and collect the records
/// that changed, in the order they changed. Delete cascades to Comments of an Entry.
/// `Err` is a rejection code: the mutation is refused, nothing it did survives (the
/// caller never publishes for an `Err`), and the caller counts it as `rejected`
/// rather than `accepted` - the same outcome a real per-mutation rejection produces,
/// so the client processes it through the ordinary rejection/rollback path.
fn apply_business(
    t: &mut Tables,
    name: &str,
    arguments: &Value,
    uppercase: bool,
) -> Result<Vec<RecordKey>, &'static str> {
    let (model, slot) = match name {
        "CreateEntry" | "Edit" | "DeleteEntry" => ("Entry", "entry"),
        "CreateComment" | "EditComment" | "DeleteComment" => ("Comment", "comment"),
        other => panic!("unknown mutation {other}"),
    };
    let arg = &arguments[slot];
    let key = key_of(model, &arg["identity"]);
    let k = encoded(&key);
    let mut changed = vec![];
    let normalize = |state: &mut Map<String, Value>| {
        if uppercase && let Some(text) = state.get("text").and_then(Value::as_str) {
            let upper = text.to_uppercase();
            state.insert("text".into(), Value::String(upper));
        }
    };
    match name {
        "CreateEntry" | "CreateComment" => {
            // The schema declares Comment.entryId as a real relation to Entry with
            // onDelete: delete (see schema.rs) - a real FK-backed store would refuse
            // an insert whose parent row is missing. Two clients racing a delete of
            // the parent against a create of the child (both accepted independently
            // by the mutation queue, which never checks against remote state) is
            // exactly the case that constraint exists to catch: without it, the
            // comment lands as a permanent orphan no future delete will ever cascade
            // into, because that delete already happened.
            if name == "CreateComment" {
                let entry_id = arg["data"]["entryId"].clone();
                let entry_key = encoded(&key_of("Entry", &json!({ "id": entry_id })));
                if !t.records.contains_key(&entry_key) {
                    return Err("comment.entry_missing");
                }
            }
            let mut state = arg["data"].as_object().cloned().unwrap_or_default();
            for (f, v) in arg["identity"].as_object().unwrap() {
                state.insert(f.clone(), v.clone());
            }
            normalize(&mut state);
            t.records.insert(k, Value::Object(state));
            changed.push(key);
        }
        "Edit" | "EditComment" => {
            if let Some(Value::Object(state)) = t.records.get_mut(&k) {
                for (f, v) in arg["patch"].as_object().unwrap() {
                    state.insert(f.clone(), v.clone());
                }
                normalize(state);
            }
            changed.push(key);
        }
        _ => {
            t.records.remove(&k);
            if model == "Entry" {
                let id = arg["identity"]["id"].clone();
                let children: Vec<String> = t
                    .records
                    .iter()
                    .filter(|(ck, v)| ck.starts_with("[\"Comment\"") && v["entryId"] == id)
                    .map(|(ck, _)| ck.clone())
                    .collect();
                for ck in children {
                    let v = t.records.remove(&ck).unwrap();
                    changed.push(key_of("Comment", &json!({"id": v["id"]})));
                }
            }
            changed.push(key);
        }
    }
    Ok(changed)
}

/// The sim answers with the contract's own response types, so a drift between
/// `crates/server/src/host.rs` and this host is a Rust compile error.
macro_rules! response {
    ($value:expr) => {
        serde_json::to_value($value).expect("host responses encode")
    };
}

impl Host for MemHost {
    fn call(
        &self,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = axton_server::HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let request: HostRequest = serde_json::from_value(request)
                .map_err(|error| format!("unsupported host request: {error}"))?;
            let mut s = self.0.lock().unwrap();
            Ok(match request {
                HostRequest::Claim { owner, client_id } => {
                    let claimed = s
                        .clients
                        .entry(client_id.clone())
                        .or_insert_with(|| Claimed {
                            client_id,
                            owner,
                            sequence: 0,
                            receipt: None,
                        })
                        .clone();
                    response!(claimed)
                }
                HostRequest::SaveReceipt {
                    owner,
                    client_id,
                    sequence,
                    receipt,
                } => {
                    s.clients.insert(
                        client_id.clone(),
                        Claimed {
                            client_id,
                            owner,
                            sequence,
                            receipt: Some(receipt),
                        },
                    );
                    response!(Acknowledged)
                }
                HostRequest::ClaimCall {
                    owner,
                    call_id,
                    request,
                } => {
                    let key = (owner, call_id);
                    if let Some((stored_request, stored_response)) = s.tables.calls.get(&key) {
                        if stored_response.is_none() {
                            return Err("Call has incomplete stored response".into());
                        }
                        response!(ClaimedCall {
                            fresh: false,
                            request: stored_request.clone(),
                            response: stored_response.clone()
                        })
                    } else {
                        s.tables.calls.insert(key, (request.clone(), None));
                        response!(ClaimedCall {
                            fresh: true,
                            request,
                            response: None
                        })
                    }
                }
                HostRequest::SaveCall {
                    owner,
                    call_id,
                    response,
                } => {
                    let Some((_, stored_response)) = s.tables.calls.get_mut(&(owner, call_id))
                    else {
                        return Err("Call not claimed".into());
                    };
                    if stored_response.is_some() {
                        return Err("Call already completed".into());
                    }
                    *stored_response = Some(response);
                    response!(Acknowledged)
                }
                HostRequest::Head { channel } => {
                    response!(Head(s.tables.heads.get(&channel).copied().unwrap_or(0)))
                }
                HostRequest::Savepoint { .. } => {
                    let snap = s.tables.clone();
                    s.savepoints.push(snap);
                    response!(Acknowledged)
                }
                HostRequest::Rollback { .. } => {
                    // A broken transaction: the infrastructure itself fails here,
                    // rather than the handler answering a business rejection.
                    // This aborts the whole delivery (see `push`'s rollback-on-error
                    // logic), so nothing this attempt did survives.
                    if s.break_next {
                        s.break_next = false;
                        s.failed += 1;
                        return Err("injected broken transaction".into());
                    }
                    // Mirrors SQL ROLLBACK TO SAVEPOINT: restores the snapshot but leaves
                    // it on the stack. The server always follows with a `release`, which
                    // is the one that pops it (mirroring RELEASE SAVEPOINT).
                    let snap = s
                        .savepoints
                        .last()
                        .cloned()
                        .expect("rollback without savepoint");
                    s.tables = snap;
                    response!(Acknowledged)
                }
                HostRequest::Release { .. } => {
                    s.savepoints.pop().expect("release without savepoint");
                    response!(Acknowledged)
                }
                HostRequest::Handle {
                    name,
                    arguments,
                    ordinal,
                    ..
                } => {
                    s.handler_calls += 1;
                    if let Some((client_id, batch_sequence)) = s.current_push.clone() {
                        s.handler_invocations
                            .push((client_id, batch_sequence, ordinal));
                    }
                    if s.fail_next {
                        s.fail_next = false;
                        s.rejected += 1;
                        return Ok(response!(Handled::Failed {
                            error: "injected failure".into(),
                        }));
                    }
                    if let Some(code) = s.reject_next.take() {
                        s.rejected += 1;
                        return Ok(response!(Handled::Rejected { rejection: code }));
                    }
                    let uppercase = std::mem::take(&mut s.uppercase_next);
                    match apply_business(&mut s.tables, &name, &arguments, uppercase) {
                        Ok(changed) => {
                            // Report every changed record (the engine dedups the ones
                            // the operations already named) and ask for one publication
                            // per channel any of them is a member of, carrying exactly
                            // the changed records that channel provides. A record with
                            // no membership is changed, stamped and read back, but
                            // published nowhere.
                            let changes: Vec<RecordRef> = changed
                                .iter()
                                .map(|key| RecordRef {
                                    model: key.model.clone(),
                                    identity: key.identity.clone(),
                                })
                                .collect();
                            let mut by_channel: BTreeMap<String, Vec<RecordRef>> = BTreeMap::new();
                            for (key, record) in changed.iter().zip(&changes) {
                                let channels: BTreeSet<String> = s
                                    .membership
                                    .get(&encoded(key))
                                    .cloned()
                                    .unwrap_or_default()
                                    .into_iter()
                                    .collect();
                                for channel in channels {
                                    by_channel.entry(channel).or_default().push(record.clone());
                                }
                            }
                            let publications = by_channel
                                .into_iter()
                                .map(|(channel, records)| PublicationIntent {
                                    channel,
                                    records: Some(records),
                                })
                                .collect();
                            s.accepted += 1;
                            response!(Handled::Settled {
                                changes,
                                publications,
                            })
                        }
                        Err(code) => {
                            s.rejected += 1;
                            response!(Handled::Rejected {
                                rejection: code.to_string(),
                            })
                        }
                    }
                }
                HostRequest::AdvanceStamp {
                    model,
                    identity_key,
                } => {
                    let key = key_from_identity_key(&model, &identity_key)?;
                    response!(Stamped(advance(&mut s.tables, &key)))
                }
                HostRequest::EnsureStamp {
                    model,
                    identity_key,
                } => {
                    let key = key_from_identity_key(&model, &identity_key)?;
                    response!(Stamped(ensure(&mut s.tables, &key)))
                }
                HostRequest::Publish {
                    channel,
                    model,
                    identity,
                    stamp,
                    ..
                } => {
                    let key = key_of(&model, &identity);
                    let cursor = publish_at(&mut s.tables, &channel, &key, stamp)?;
                    response!(Published { cursor, stamp })
                }
                HostRequest::Scan {
                    channel,
                    after,
                    limit,
                } => {
                    let mut rows: Vec<&Invalidation> = s
                        .tables
                        .invalidations
                        .iter()
                        .filter(|((c, _), row)| *c == channel && row.cursor > after)
                        .map(|(_, row)| row)
                        .collect();
                    rows.sort_by_key(|row| row.cursor);
                    let scanned: Scanned = rows
                        .into_iter()
                        .take(limit as usize)
                        .map(|row| {
                            // The cursor is delivery progress; the stamp is the record's
                            // current version, read from the stamp table in the same
                            // snapshot the loader reads, so content and stamp agree even
                            // when the record advanced after this row was written.
                            let key = key_of(&row.model, &row.identity);
                            let current = s
                                .tables
                                .stamps
                                .get(&encoded(&key))
                                .map(|s| s.value)
                                .unwrap_or(row.stamp);
                            ContractInvalidation {
                                channel: channel.clone(),
                                cursor: row.cursor,
                                model: row.model.clone(),
                                identity: row.identity.clone(),
                                identity_key: row.identity_key.clone(),
                                stamp: current,
                            }
                        })
                        .collect();
                    response!(scanned)
                }
                HostRequest::Load {
                    model, identities, ..
                } => {
                    // A record marked to fail makes every load naming it fail; the
                    // mark is consumed by the single-identity retry, so exactly
                    // that record ends as an error change.
                    let keys: Vec<String> = identities
                        .iter()
                        .map(|identity| encoded(&key_of(&model, identity)))
                        .collect();
                    let single = keys.len() == 1;
                    if let Some(k) = keys.iter().find(|k| s.fail_load.contains(*k)).cloned() {
                        if single {
                            s.fail_load.remove(&k);
                        }
                        return Ok(response!(Loaded::Failed {
                            error: "sim load failure".into()
                        }));
                    }
                    if let Some(k) = keys.iter().find(|k| s.refuse_load.contains(*k)).cloned() {
                        if single {
                            s.refuse_load.remove(&k);
                        }
                        return Ok(response!(Loaded::Refused {
                            rejection: "sim.refused".into()
                        }));
                    }
                    // Loads name no channel: the record exists or it does not, for
                    // every delivery path alike.
                    let rows: Vec<Option<Value>> = identities
                        .iter()
                        .map(|identity| {
                            let key = key_of(&model, identity);
                            s.tables.records.get(&encoded(&key)).map(|v| {
                                let mut m: Map<String, Value> = v.as_object().unwrap().clone();
                                if model == "Entry" {
                                    m.entry("note").or_insert(Value::Null);
                                }
                                Value::Object(m)
                            })
                        })
                        .collect();
                    response!(Loaded::Rows(rows))
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{self, entry_key};
    use axton_core::{PullPage, PullRequest, PushReceipt};

    fn push_bytes(client_id: &str, sequence: u64, mutation: &axton_client::Mutation) -> Vec<u8> {
        let m = serde_json::to_value(mutation).unwrap();
        let ops = &m["operations"];
        let body = json!({"clientId":client_id,"batchSequence":sequence,"models":schema::declared_models(),"mutations":[{"ordinal":1,"name":mutation.name,"version":1,"operations":ops}]});
        axton_core::PushRequest::decode(axton_core::canonical_json(&body).unwrap().as_bytes())
            .unwrap()
            .encode()
            .unwrap()
    }

    fn pull(host: &MemHost, channel: &str, from: u64) -> PullPage {
        let req = PullRequest {
            cursors: BTreeMap::from([(channel.to_string(), from)]),
            models: schema::declared_models(),
        }
        .encode()
        .unwrap();
        PullPage::decode(host.pull("u", &req).unwrap().as_bytes()).unwrap()
    }

    #[test]
    fn push_allocates_one_stamp_and_pull_delivers_it_on_every_channel() {
        let host = MemHost::new();
        host.set_membership(&entry_key("e1"), &["a", "b"]);
        let receipt = host
            .push("u", &push_bytes("c1", 1, &schema::create_entry("e1", "hi")))
            .unwrap();
        let receipt = PushReceipt::decode(receipt.as_bytes()).unwrap();
        assert_eq!(receipt.rejections.len(), 0);
        assert_eq!(receipt.records.len(), 1, "the created entry is read back");
        assert_eq!(receipt.records[0].model, "Entry");
        assert_eq!(receipt.records[0].stamp, 1);
        assert_eq!(receipt.records[0].state["text"], "hi");
        assert_eq!(receipt.records[0].state["note"], Value::Null);
        assert_eq!(host.head("a"), 1);
        assert_eq!(host.head("b"), 1);
        assert_eq!(
            host.stamp(&entry_key("e1")),
            1,
            "one change is one stamp, however many channels distribute it"
        );
        assert_eq!(host.channel_stamp("a", &entry_key("e1")), Some(1));
        assert_eq!(host.channel_stamp("b", &entry_key("e1")), Some(1));
        assert_eq!(host.handler_calls(), 1);
        // Duplicate push returns the stored receipt without a handler call.
        let again = host
            .push("u", &push_bytes("c1", 1, &schema::create_entry("e1", "hi")))
            .unwrap();
        assert_eq!(PushReceipt::decode(again.as_bytes()).unwrap(), receipt);
        assert_eq!(host.handler_calls(), 1);
        // A gap is refused.
        assert_eq!(
            host.push("u", &push_bytes("c1", 3, &schema::edit("e1", "x")))
                .unwrap_err(),
            "gap"
        );
        // Pull on b sees the record at the same stamp the receipt carried.
        let page = pull(&host, "b", 0);
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].stamp, 1);
        assert_eq!(page.changes[0].state["text"], "hi");
        assert_eq!(page.cursors["b"].to, 1);
    }

    #[test]
    fn a_change_with_no_membership_is_read_back_but_published_nowhere() {
        let host = MemHost::new();
        host.set_membership(&entry_key("e1"), &[]);
        let receipt = PushReceipt::decode(
            host.push("u", &push_bytes("c1", 1, &schema::create_entry("e1", "hi")))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(receipt.rejections.len(), 0);
        assert_eq!(receipt.records[0].stamp, 1);
        assert_eq!(host.stamp(&entry_key("e1")), 1);
        assert_eq!(host.head("a"), 0, "no channel was told");
        assert!(host.channel_records("a").is_empty());
    }

    #[test]
    fn scan_carries_the_current_stamp_and_republication_does_not_advance_it() {
        let host = MemHost::new();
        host.set_state(
            &entry_key("e1"),
            Some(json!({"id":"e1","text":"v1","note":null})),
        );
        host.notify(&entry_key("e1"), &["a"]); // stamp 1, a:1
        host.set_state(
            &entry_key("e1"),
            Some(json!({"id":"e1","text":"v2","note":null})),
        );
        host.notify(&entry_key("e1"), &["b"]); // stamp 2, b:1; a's row still says 1
        assert_eq!(host.channel_stamp("a", &entry_key("e1")), Some(1));
        let page = pull(&host, "a", 0);
        assert_eq!(
            page.changes[0].stamp, 2,
            "the row's stamp is the record's now"
        );
        assert_eq!(page.changes[0].state["text"], "v2");
        assert_eq!(page.cursors["a"].to, 1, "the cursor is the row's own");
        host.ensure_publish(&entry_key("e1"), "c");
        assert_eq!(
            host.stamp(&entry_key("e1")),
            2,
            "republication never advances"
        );
        assert_eq!(host.channel_stamp("c", &entry_key("e1")), Some(2));
        assert_eq!(host.head("c"), 1);
        // A record with no stamp at all gets one on first publication, once.
        host.ensure_publish(&entry_key("e2"), "c");
        host.ensure_publish(&entry_key("e2"), "c");
        assert_eq!(host.stamp(&entry_key("e2")), 1);
        assert_eq!(host.head("c"), 3);
        let accounting: BTreeMap<String, (u64, u64, bool)> = host
            .stamp_accounting()
            .into_iter()
            .map(|(k, s, a, i)| (encoded(&k), (s, a, i)))
            .collect();
        assert_eq!(accounting[&encoded(&entry_key("e1"))], (2, 2, false));
        assert_eq!(accounting[&encoded(&entry_key("e2"))], (1, 0, true));
    }

    #[test]
    fn rejection_rolls_back_one_mutation_and_failure_aborts_the_batch() {
        let host = MemHost::new();
        host.set_membership(&entry_key("e1"), &["a"]);
        host.push("u", &push_bytes("c1", 1, &schema::create_entry("e1", "hi")))
            .unwrap();
        host.reject_next("entry.denied");
        let r = PushReceipt::decode(
            host.push("u", &push_bytes("c1", 2, &schema::edit("e1", "no")))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(r.rejections.len(), 1);
        assert_eq!(r.rejections[0].code, "entry.denied");
        assert!(
            r.records.is_empty(),
            "a rejected mutation reports no authority"
        );
        assert_eq!(host.state(&entry_key("e1")).unwrap()["text"], "hi");
        assert_eq!(host.head("a"), 1);
        assert_eq!(
            host.stamp(&entry_key("e1")),
            1,
            "the rollback undid nothing more"
        );
        // A handler failure is a rejection of that one mutation: the batch
        // commits, it just carries `handler.failed` in place of a business code.
        host.fail_next();
        let failed = PushReceipt::decode(
            host.push("u", &push_bytes("c1", 3, &schema::edit("e1", "boom")))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(failed.rejections.len(), 1);
        assert_eq!(failed.rejections[0].code, "handler.failed");
        assert_eq!(host.state(&entry_key("e1")).unwrap()["text"], "hi");
        assert_eq!(host.head("a"), 1);
        assert_eq!(host.stamp(&entry_key("e1")), 1);
        assert_eq!(
            host.savepoint_depth(),
            0,
            "the rolled-back savepoint must not leak"
        );
        // A broken transaction (a host infrastructure error on `rollback`, not
        // a handler rejection) aborts the whole delivery: nothing is
        // committed and the client's retry with the same bytes is expected to
        // reach the handler again. `rollback` only runs after a refused
        // mutation, so this pairs `break_next` with a rejection to reach it.
        host.reject_next("entry.denied");
        host.break_next();
        assert!(
            host.push("u", &push_bytes("c1", 4, &schema::edit("e1", "boom")))
                .is_err()
        );
        assert_eq!(host.state(&entry_key("e1")).unwrap()["text"], "hi");
        assert_eq!(host.head("a"), 1);
        assert_eq!(host.stamp(&entry_key("e1")), 1);
        assert_eq!(
            host.savepoint_depth(),
            0,
            "the failed batch's savepoint must not leak"
        );
        // The failed batch left no receipt, so sequence 4 is still next.
        let ok = PushReceipt::decode(
            host.push("u", &push_bytes("c1", 4, &schema::edit("e1", "yes")))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(ok.rejections.len(), 0);
        assert_eq!(ok.records[0].stamp, 2);
        assert_eq!(ok.records[0].state["text"], "yes");
        assert_eq!(host.state(&entry_key("e1")).unwrap()["text"], "yes");
    }

    #[test]
    fn deleting_an_entry_cascades_to_its_comments_and_reports_each() {
        let host = MemHost::new();
        host.set_membership(&entry_key("e1"), &["a"]);
        host.set_membership(&schema::comment_key("c1"), &["a"]);
        host.push("u", &push_bytes("c1", 1, &schema::create_entry("e1", "hi")))
            .unwrap();
        host.push(
            "u",
            &push_bytes("c1", 2, &schema::create_comment("c1", "e1", "yo")),
        )
        .unwrap();
        let r = PushReceipt::decode(
            host.push("u", &push_bytes("c1", 3, &schema::delete_entry("e1")))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert!(host.state(&entry_key("e1")).is_none());
        assert!(host.state(&schema::comment_key("c1")).is_none());
        assert_eq!(
            r.records.len(),
            2,
            "the cascaded comment is a change the handler reported"
        );
        assert!(r.records.iter().all(|c| c.state.is_null()));
        assert_eq!(host.stamp(&entry_key("e1")), 2);
        assert_eq!(host.stamp(&schema::comment_key("c1")), 2);
        assert_eq!(host.head("a"), 4);
        let page = pull(&host, "a", 2);
        assert_eq!(page.changes.len(), 2);
        assert!(page.changes.iter().all(|c| c.state.is_null()));
        assert!(page.changes.iter().all(|c| c.stamp == 2));
    }

    #[test]
    fn create_comment_against_a_missing_entry_is_deterministically_rejected() {
        let host = MemHost::new();
        host.set_membership(&schema::comment_key("c1"), &["a"]);
        // No Entry e1 was ever created: the schema's onDelete: delete relation means
        // a real FK-backed store would refuse this insert outright.
        let r = PushReceipt::decode(
            host.push(
                "u",
                &push_bytes("c1", 1, &schema::create_comment("c1", "e1", "hi")),
            )
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(r.rejections.len(), 1);
        assert_eq!(r.rejections[0].code, "comment.entry_missing");
        assert!(host.state(&schema::comment_key("c1")).is_none());
        assert_eq!(host.rejected(), 1);
        assert_eq!(host.accepted(), 0);
        assert_eq!(
            host.handler_calls(),
            host.accepted() + host.rejected() + host.failed(),
            "no_mutation_executes_twice must hold for a rejection too"
        );
    }

    #[test]
    fn load_is_membership_blind() {
        let host = MemHost::new();
        host.set_membership(&entry_key("e1"), &["b"]);
        host.set_state(
            &entry_key("e1"),
            Some(json!({"id":"e1","text":"in b","note":null})),
        );
        // Published on a channel outside its membership: the content is the same on
        // every delivery path, membership only decides where it is routed.
        host.notify(&entry_key("e1"), &["a", "b"]);
        let page_a = pull(&host, "a", 0);
        assert_eq!(page_a.changes.len(), 1);
        assert_eq!(page_a.changes[0].state["text"], "in b");
        let page_b = pull(&host, "b", 0);
        assert_eq!(page_b.changes[0].state["text"], "in b");
        assert_eq!(page_a.changes[0].stamp, page_b.changes[0].stamp);
    }
}
