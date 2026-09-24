//! One command/value contract shared by native language bridges.
use axton_client::*;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct RuntimeHost {
    next: u64,
    clients: BTreeMap<u64, Entry>,
}
struct Entry {
    client: Client<SqliteStore>,
    cycle: SyncCycle,
    connection: ConnectionDriver,
    live: LiveSession,
}
impl RuntimeHost {
    pub fn call(&mut self, request: Value) -> Result<Value> {
        let op = text(&request, "op")?;
        if op == "open" {
            // `owner` and `migration` may still be sent by language packages; the
            // row-based client keeps neither, so both are accepted and ignored.
            let discard = request
                .get("discardPending")
                .map(|v| {
                    v.as_bool()
                        .ok_or_else(|| invalid("discardPending must be bool"))
                })
                .transpose()?
                .unwrap_or(false);
            let client = Client::open_at(
                text(&request, "path")?,
                Schema::from_value(request["schema"].clone())?,
                Box::new(|p| SqliteStore::open(p)),
                discard,
            )?;
            self.next = self
                .next
                .checked_add(1)
                .ok_or_else(|| invalid("handle exhausted"))?;
            let handle = self.next;
            let generation = client.generation();
            let client_id = client.client_id().to_string();
            self.clients.insert(
                handle,
                Entry {
                    client,
                    cycle: SyncCycle::default(),
                    connection: ConnectionDriver::default(),
                    live: LiveSession::default(),
                },
            );
            let schema_state = schema_json(self.clients[&handle].client.schema_state());
            return Ok(
                json!({"value":{"handle":handle,"clientId":client_id,"schema":schema_state},"changed":false,"generation":generation}),
            );
        }
        let id = read_counter(&request["handle"], true)?;
        if op == "close" {
            self.clients
                .remove(&id)
                .ok_or_else(|| invalid("client_closed"))?;
            return Ok(json!({"value":null,"changed":false}));
        }
        let e = self
            .clients
            .get_mut(&id)
            .ok_or_else(|| invalid("client_closed"))?;
        let generation = e.client.generation();
        if request["transaction"] == true && !e.client.session_active() {
            return Err(invalid("transaction_closed"));
        }
        let value = match op {
            "begin" => {
                e.client.begin_session()?;
                Value::Null
            }
            "commit" => {
                e.client.commit_session()?;
                Value::Null
            }
            "rollback" => {
                e.client.rollback_session()?;
                Value::Null
            }
            "savepoint" => {
                e.client.session_savepoint()?;
                Value::Null
            }
            "release" => {
                e.client.session_release()?;
                Value::Null
            }
            "rollbackSavepoint" => {
                e.client.session_rollback_savepoint()?;
                Value::Null
            }
            "read" => {
                let key: RecordKey = serde_json::from_value(request["key"].clone())?;
                if e.client.session_active() {
                    e.client.session(|tx| tx.read(&key))?
                } else {
                    e.client.read(&key)?
                }
                .unwrap_or(Value::Null)
            }
            "query" => {
                let model = text(&request, "model")?;
                let filter = request.get("filter").cloned().unwrap_or(json!({}));
                serde_json::to_value(if e.client.session_active() {
                    e.client.session(|tx| tx.query(model, &filter))?
                } else {
                    e.client.query(model, &filter)?
                })?
            }
            "sql" => {
                let sql = text(&request, "sql")?;
                let parameters = request["parameters"]
                    .as_array()
                    .ok_or_else(|| invalid("SQL parameters must be array"))?;
                serde_json::to_value(if e.client.session_active() {
                    e.client.session_sql(sql, parameters)?
                } else {
                    e.client.read_sql(sql, parameters)?
                })?
            }
            "querySpec" => {
                let model = text(&request, "model")?;
                let spec: QuerySpec = serde_json::from_value(request["query"].clone())?;
                serde_json::to_value(if e.client.session_active() {
                    e.client.session(|tx| tx.query_spec(model, &spec))?
                } else {
                    e.client.query_spec(model, &spec)?
                })?
            }
            "related" => {
                let key: RecordKey = serde_json::from_value(request["key"].clone())?;
                let name = text(&request, "relation")?;
                if e.client.session_active() {
                    e.client.session(|tx| tx.related(&key, name))?
                } else {
                    e.client.related(&key, name)?
                }
                .unwrap_or(Value::Null)
            }
            "referencing" => {
                let key: RecordKey = serde_json::from_value(request["key"].clone())?;
                let name = text(&request, "relation")?;
                let source = text(&request, "source")?;
                serde_json::to_value(if e.client.session_active() {
                    e.client.session(|tx| tx.referencing(&key, source, name))?
                } else {
                    e.client.referencing(&key, source, name)?
                })?
            }
            "enqueue" => {
                let mutation: Mutation = serde_json::from_value(request["mutation"].clone())?;
                let ordinal = if e.client.session_active() {
                    e.client.session(|tx| tx.enqueue(mutation))?
                } else {
                    e.client.transaction(|tx| tx.enqueue(mutation))?
                };
                json!(ordinal)
            }
            "submitAction" => {
                let name = text(&request, "name")?;
                let version = read_counter(&request["version"], true)?;
                let submitted = e
                    .client
                    .submit_action(name, version, request["args"].clone())?;
                json!({"callId":submitted.call_id,"ordinal":submitted.ordinal})
            }
            "direct" => {
                let operation: Operation = serde_json::from_value(request["operation"].clone())?;
                if e.client.session_active() {
                    e.client.session(|tx| tx.direct(operation))?
                } else {
                    e.client.transaction(|tx| tx.direct(operation))?
                };
                Value::Null
            }
            "channel" => {
                let channel = text(&request, "channel")?.to_string();
                let subscribed = request["subscribed"]
                    .as_bool()
                    .ok_or_else(|| invalid("subscribed must be bool"))?;
                if e.client.session_active() {
                    e.client.session(|tx| tx.set_channel(channel, subscribed))?
                } else {
                    e.client
                        .transaction(|tx| tx.set_channel(channel, subscribed))?
                };
                Value::Null
            }
            _ => {
                if e.client.session_active() {
                    return Err(invalid("client transaction active"));
                }
                match op {
                    "connection" => {
                        let connection = &mut e.connection;
                        let now = read_now(&request)?;
                        match text(&request, "event")? {
                            "start" => connection.start(now),
                            "stop" => connection.stop(),
                            "pause" => connection.pause(),
                            "resume" => connection.resume(now),
                            "wake" => connection.wake(),
                            "success" => connection.complete(true, now, 0),
                            "failure" => connection.complete(false, now, read_entropy(&request)?),
                            "next" => {}
                            _ => return Err(invalid("unknown connection event")),
                        }
                        if request["event"] == "next" {
                            serde_json::to_value(connection.next(now))?
                        } else {
                            Value::Null
                        }
                    }
                    "live" => {
                        let event: LiveEvent = serde_json::from_value(request.clone())
                            .map_err(|e| invalid(format!("invalid live event: {e}")))?;
                        let now = read_now(&request)?;
                        let entropy = read_entropy(&request)?;
                        serde_json::to_value(e.live.handle(&mut e.client, event, now, entropy)?)?
                    }
                    "startSync" => {
                        match request.get("pushOnly") {
                            None | Some(Value::Bool(false)) => e.cycle.restart(),
                            Some(Value::Bool(true)) => e.cycle.restart_push_only(),
                            _ => return Err(invalid("pushOnly must be bool")),
                        }
                        Value::Null
                    }
                    "next" => serde_json::to_value(e.cycle.next(&mut e.client)?)?,
                    "complete" => serde_json::to_value(e.cycle.complete(
                        &mut e.client,
                        serde_json::to_string(&request["response"])?.as_bytes(),
                    )?)?,
                    "freeze" => match e.client.freeze()? {
                        Some(bytes) => {
                            json!(String::from_utf8(bytes).map_err(|_| invalid("utf8"))?)
                        }
                        None => Value::Null,
                    },
                    "prepareAction" => {
                        let prepared = e.client.prepare_action(
                            text(&request, "name")?,
                            read_counter(&request["version"], true)?,
                            request["args"].clone(),
                        )?;
                        json!({"callId":prepared.call.call_id,"body":String::from_utf8(prepared.encode()?).map_err(|_|invalid("utf8"))?})
                    }
                    "applyActionResponse" => {
                        let body = text(&request, "body")?;
                        let response = serde_json::to_vec(&request["response"])?;
                        serde_json::to_value(
                            e.client
                                .apply_action_response_bytes(body.as_bytes(), &response)?,
                        )?
                    }
                    "ack" => {
                        let bytes = serde_json::to_vec(&request["receipt"])?;
                        let receipt = if request["receipt"].get("completions").is_some() {
                            PushReceipt::decode_action_envelope(&bytes)?
                        } else {
                            PushReceipt::decode(&bytes)?
                        };
                        serde_json::to_value(
                            e.client
                                .acknowledge(read_counter(&request["sequence"], true)?, receipt)?,
                        )?
                    }
                    "pull" => {
                        let page =
                            PullPage::decode(serde_json::to_string(&request["page"])?.as_bytes())?;
                        serde_json::to_value(e.client.apply_page(page)?)?
                    }
                    "readiness" => {
                        let readiness: Readiness =
                            serde_json::from_value(request["state"].clone())?;
                        e.client.set_readiness(text(&request, "key")?, readiness)?;
                        Value::Null
                    }
                    "drop" => {
                        json!({"completions":e.client.drop_action(read_counter(&request["ordinal"], true)?)?})
                    }
                    "dismiss" => {
                        e.client
                            .dismiss_rejection(read_counter(&request["ordinal"], true)?)?;
                        Value::Null
                    }
                    "recordStatus" => {
                        let key: RecordKey = serde_json::from_value(request["key"].clone())?;
                        e.client.record_status(&key)?
                    }
                    "tasks" => json!(e.client.pending_tasks()?),
                    "task" => {
                        let handlers: Vec<String> =
                            serde_json::from_value(request["handlers"].clone())?;
                        json!(e.client.next_task(&handlers)?)
                    }
                    "outcome" => {
                        let error = match &request["error"] {
                            Value::Null => None,
                            Value::String(reason) => Some(reason.as_str()),
                            _ => return Err(invalid("outcome error must be a string or null")),
                        };
                        e.client.outcome(text(&request, "key")?, error)?;
                        Value::Null
                    }
                    "status" => {
                        json!({"clientId":e.client.client_id(),"pending":e.client.pending_count()?,"beforeImages":e.client.before_image_count()?,"cursors":e.client.subscriptions()?.into_iter().collect::<BTreeMap<_,_>>(),"channels":e.client.desired_channels()?,"rejections":e.client.rejections()?,"schema":schema_json(e.client.schema_state())})
                    }
                    "rebuild" => {
                        let discard = request
                            .get("discardPending")
                            .map(|v| {
                                v.as_bool()
                                    .ok_or_else(|| invalid("discardPending must be bool"))
                            })
                            .transpose()?
                            .unwrap_or(false);
                        let report = e.client.rebuild(discard)?;
                        e.cycle = SyncCycle::default();
                        e.live = LiveSession::default();
                        json!({"oldFile":report.old_file,"newFile":report.new_file,"reason":report.reason,"leftPending":report.left_pending,"leftDirect":report.left_direct,"abandonedCalls":abandoned_json(&report.abandoned_calls)})
                    }
                    _ => return Err(invalid(format!("unknown client command {op}"))),
                }
            }
        };
        Ok(
            json!({"value":value,"changed":generation!=e.client.generation(),"changedTables":e.client.last_changed(),"generation":e.client.generation()}),
        )
    }
}
/// The schema check's outcome as language packages report it in `status()`.
fn schema_json(state: &SchemaState) -> Value {
    json!({
        "rebuilt": state.rebuilt,
        "pending": state.pending.as_ref().map(|p| json!({"oldFile":p.old_file,"reason":p.reason,"pending":p.pending,"direct":p.direct})),
        "lastRebuild": state.last_rebuild.as_ref().map(|r| json!({"oldFile":r.old_file,"newFile":r.new_file,"reason":r.reason,"leftPending":r.left_pending,"leftDirect":r.left_direct,"abandonedCalls":abandoned_json(&r.abandoned_calls)})),
    })
}
fn abandoned_json(calls: &[AbandonedCall]) -> Vec<Value> {
    calls
        .iter()
        .map(|call| json!({"callId":call.call_id,"frozen":call.frozen}))
        .collect()
}
fn read_now(request: &Value) -> Result<u64> {
    request
        .get("now")
        .map(|v| read_counter(v, false))
        .transpose()
        .map(|now| now.unwrap_or(0))
}
fn read_entropy(request: &Value) -> Result<u64> {
    request
        .get("entropy")
        .map(|v| read_counter(v, false))
        .transpose()
        .map(|entropy| entropy.unwrap_or(0))
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| invalid(format!("{key} must be string")))
}
