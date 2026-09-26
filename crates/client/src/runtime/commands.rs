//! The command set a task or a transaction command names by `kind`
//! ([#134](https://github.com/zanminwang/axton/issues/134)).
//!
//! These are the commands of the former `RuntimeHost`, minus its session and
//! lifecycle commands, with the same fields and answers. A task runs only while
//! no application transaction is open, so its reads use the committed reader
//! and its writes own their own local transaction. The lane and direct-call
//! commands (`connection`, `downlink`, `startSync`, `prepareAction`, …) keep
//! their host-driven shape and read `now` / `entropy` from the command; the
//! runtime owns those lifecycles now (`connect`, `invoke`,
//! `runPrerequisites`), and checkpoint 4 of #134 deletes them.
use super::lanes::Lanes;
use crate::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Execute one task command against the committed client.
pub(super) fn execute<S: ClientStore + 'static>(
    client: &mut Client<S>,
    lanes: &mut Lanes,
    command: &Value,
) -> Result<Value> {
    let kind = text(command, "kind")?;
    Ok(match kind {
        "read" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            client.read(&key)?.unwrap_or(Value::Null)
        }
        "query" => {
            let filter = command.get("filter").cloned().unwrap_or(json!({}));
            serde_json::to_value(client.query(text(command, "model")?, &filter)?)?
        }
        "sql" => {
            serde_json::to_value(client.read_sql(text(command, "sql")?, parameters(command)?)?)?
        }
        "querySpec" => {
            let spec: QuerySpec = serde_json::from_value(command["query"].clone())?;
            serde_json::to_value(client.query_spec(text(command, "model")?, &spec)?)?
        }
        "related" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            client
                .related(&key, text(command, "relation")?)?
                .unwrap_or(Value::Null)
        }
        "referencing" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            serde_json::to_value(client.referencing(
                &key,
                text(command, "source")?,
                text(command, "relation")?,
            )?)?
        }
        "enqueue" => {
            let mutation: Mutation = serde_json::from_value(command["mutation"].clone())?;
            json!(client.transaction(|tx| tx.enqueue(mutation))?)
        }
        "direct" => {
            let operation: Operation = serde_json::from_value(command["operation"].clone())?;
            client.transaction(|tx| tx.direct(operation))?;
            Value::Null
        }
        "channel" => {
            let (channel, subscribed) = channel(command)?;
            client.transaction(|tx| tx.set_channel(channel, subscribed))?;
            Value::Null
        }
        "submitAction" => {
            let submitted = client.submit_action_with_options(
                text(command, "name")?,
                read_counter(&command["version"], true)?,
                command["args"].clone(),
                action_options(command)?,
            )?;
            json!({"callId":submitted.call_id,"ordinal":submitted.ordinal})
        }
        // The Scope commands behind the SDK subscription handles: each owns its
        // own local transaction. An uninitialized boundary answers as `null`,
        // never as zero ([#150](https://github.com/zanminwang/axton/issues/150)).
        "scopeSubscribe" => {
            serde_json::to_value(client.ensure_subscription(text(command, "scope")?)?)?
        }
        "scopeState" => match client.subscription_state(text(command, "scope")?)? {
            Some(state) => serde_json::to_value(state)?,
            None => Value::Null,
        },
        // The durable load of a Scope's history
        // ([#151](https://github.com/zanminwang/axton/issues/151)).
        "scopeBootstrap" => serde_json::to_value(client.request_bootstrap(
            text(command, "scope")?,
            read_counter(&command["subscriptionId"], true)?,
        )?)?,
        "scopeBootstrapState" => serde_json::to_value(client.bootstrap_state(
            text(command, "scope")?,
            read_counter(&command["subscriptionId"], true)?,
        )?)?,
        "scopeUnsubscribe" => {
            let scope = text(command, "scope")?.to_string();
            let subscription_id = read_counter(&command["subscriptionId"], true)?;
            json!({"removed":client.remove_subscription(&scope, subscription_id)?})
        }
        "connection" => {
            let connection = &mut lanes.connection;
            let now = read_now(command)?;
            let event = text(command, "event")?;
            match event {
                "start" => connection.start(now),
                "stop" => connection.stop(),
                "pause" => connection.pause(),
                "resume" => connection.resume(now),
                "wake" => connection.wake(),
                "success" => connection.complete(true, now, 0),
                "failure" => connection.complete(false, now, read_entropy(command)?),
                "next" => {}
                _ => return Err(invalid("unknown connection event")),
            }
            if event == "next" {
                serde_json::to_value(connection.next(now))?
            } else {
                Value::Null
            }
        }
        "downlink" => {
            let event: DownlinkEvent = serde_json::from_value(command.clone())
                .map_err(|e| invalid(format!("invalid downlink event: {e}")))?;
            let now = read_now(command)?;
            let entropy = read_entropy(command)?;
            serde_json::to_value(lanes.downlink.handle(client, event, now, entropy)?)?
        }
        "startSync" => {
            match command.get("pushOnly") {
                None | Some(Value::Bool(false)) => lanes.cycle.restart(),
                Some(Value::Bool(true)) => lanes.cycle.restart_push_only(),
                _ => return Err(invalid("pushOnly must be bool")),
            }
            Value::Null
        }
        "next" => serde_json::to_value(lanes.cycle.next(client)?)?,
        "complete" => serde_json::to_value(lanes.cycle.complete(
            client,
            serde_json::to_string(&command["response"])?.as_bytes(),
        )?)?,
        "freeze" => match client.freeze()? {
            Some(bytes) => json!(String::from_utf8(bytes).map_err(|_| invalid("utf8"))?),
            None => Value::Null,
        },
        "prepareAction" => {
            let prepared = client.prepare_action_with_options(
                text(command, "name")?,
                read_counter(&command["version"], true)?,
                command["args"].clone(),
                action_options(command)?,
            )?;
            json!({"callId":prepared.call.call_id,"body":String::from_utf8(prepared.encode()?).map_err(|_|invalid("utf8"))?})
        }
        // Query once (#158): Rust decides and fences; the host executes the
        // prepared body and fans the outcome out.
        "queryOnce" => {
            let refresh = match command.get("refresh") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(refresh)) => *refresh,
                _ => return Err(invalid("refresh must be bool")),
            };
            let decision = client.begin_query_once(
                text(command, "name")?,
                read_counter(&command["version"], true)?,
                &command["args"],
                &QueryOnceOptions {
                    store: action_options(command)?.store,
                    refresh,
                },
            )?;
            match decision {
                QueryOnce::Cached { result } => json!({"decision":"cached","result":result}),
                QueryOnce::Join { flight_id } => json!({"decision":"join","flightId":flight_id}),
                QueryOnce::Fetch { flight_id, request } => {
                    json!({"decision":"fetch","flightId":flight_id,"callId":request.call.call_id,"body":String::from_utf8(request.encode()?).map_err(|_|invalid("utf8"))?})
                }
            }
        }
        "finishQueryOnce" => {
            let response = serde_json::to_vec(&command["response"])?;
            serde_json::to_value(client.finish_query_once(text(command, "flightId")?, &response)?)?
        }
        "failQueryOnce" => {
            json!({"released":client.fail_query_once(text(command, "flightId")?)})
        }
        "invalidateQueryOnce" => {
            client.invalidate_query_once(
                text(command, "name")?,
                read_counter(&command["version"], true)?,
                &command["args"],
            )?;
            Value::Null
        }
        "applyActionResponse" => {
            let body = text(command, "body")?;
            let response = serde_json::to_vec(&command["response"])?;
            serde_json::to_value(client.apply_action_response_bytes(body.as_bytes(), &response)?)?
        }
        "ack" => {
            let bytes = serde_json::to_vec(&command["receipt"])?;
            let receipt = if command["receipt"].get("completions").is_some() {
                PushReceipt::decode_action_envelope(&bytes)?
            } else {
                PushReceipt::decode(&bytes)?
            };
            serde_json::to_value(
                client.acknowledge(read_counter(&command["sequence"], true)?, receipt)?,
            )?
        }
        "pull" => {
            let page = PullPage::decode(serde_json::to_string(&command["page"])?.as_bytes())?;
            serde_json::to_value(client.apply_page(page)?)?
        }
        "readiness" => {
            let readiness: Readiness = serde_json::from_value(command["state"].clone())?;
            client.set_readiness(text(command, "key")?, readiness)?;
            Value::Null
        }
        "drop" => {
            json!({"completions":client.drop_action(read_counter(&command["ordinal"], true)?)?})
        }
        "dismiss" => {
            client.dismiss_rejection(read_counter(&command["ordinal"], true)?)?;
            Value::Null
        }
        "recordStatus" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            client.record_status(&key)?
        }
        "tasks" => json!(client.pending_tasks()?),
        "task" => {
            let handlers: Vec<String> = serde_json::from_value(command["handlers"].clone())?;
            json!(client.next_task(&handlers)?)
        }
        "outcome" => {
            let error = match &command["error"] {
                Value::Null => None,
                Value::String(reason) => Some(reason.as_str()),
                _ => return Err(invalid("outcome error must be a string or null")),
            };
            client.outcome(text(command, "key")?, error)?;
            Value::Null
        }
        "status" => {
            json!({"clientId":client.client_id(),"pending":client.pending_count()?,"beforeImages":client.before_image_count()?,"cursors":client.subscriptions()?.into_iter().collect::<BTreeMap<_,_>>(),"channels":client.desired_channels()?,"rejections":client.rejections()?,"schema":schema_json(client.schema_state())})
        }
        "rebuild" => {
            let discard = command
                .get("discardPending")
                .map(|v| {
                    v.as_bool()
                        .ok_or_else(|| invalid("discardPending must be bool"))
                })
                .transpose()?
                .unwrap_or(false);
            let report = client.rebuild(discard)?;
            // The push cycle starts over with the fresh replica; the worker
            // forgets the old one but keeps its intent and its identifier
            // allocators, so no answer to old I/O can match new I/O (#162).
            lanes.cycle = SyncCycle::default();
            lanes.downlink.reset_for_rebuild();
            json!({"oldFile":report.old_file,"newFile":report.new_file,"reason":report.reason,"leftPending":report.left_pending,"leftDirect":report.left_direct,"abandonedCalls":abandoned_json(&report.abandoned_calls)})
        }
        _ => return Err(invalid(format!("unknown client command {kind}"))),
    })
}

/// Execute one command of the open application transaction inside its
/// session. Only reads and local writes are admitted there; the savepoint
/// commands are the transaction's own.
pub(super) fn execute_in_session<S: ClientStore>(
    client: &mut Client<S>,
    command: &Value,
) -> Result<Value> {
    let kind = command["kind"].as_str().unwrap_or_default();
    Ok(match kind {
        "read" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            client.session(|tx| tx.read(&key))?.unwrap_or(Value::Null)
        }
        "query" => {
            let model = text(command, "model")?;
            let filter = command.get("filter").cloned().unwrap_or(json!({}));
            serde_json::to_value(client.session(|tx| tx.query(model, &filter))?)?
        }
        "sql" => {
            serde_json::to_value(client.session_sql(text(command, "sql")?, parameters(command)?)?)?
        }
        "querySpec" => {
            let model = text(command, "model")?;
            let spec: QuerySpec = serde_json::from_value(command["query"].clone())?;
            serde_json::to_value(client.session(|tx| tx.query_spec(model, &spec))?)?
        }
        "related" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            let name = text(command, "relation")?;
            client
                .session(|tx| tx.related(&key, name))?
                .unwrap_or(Value::Null)
        }
        "referencing" => {
            let key: RecordKey = serde_json::from_value(command["key"].clone())?;
            let name = text(command, "relation")?;
            let source = text(command, "source")?;
            serde_json::to_value(client.session(|tx| tx.referencing(&key, source, name))?)?
        }
        "direct" => {
            let operation: Operation = serde_json::from_value(command["operation"].clone())?;
            client.session(|tx| tx.direct(operation))?;
            Value::Null
        }
        "enqueue" => {
            let mutation: Mutation = serde_json::from_value(command["mutation"].clone())?;
            json!(client.session(|tx| tx.enqueue(mutation))?)
        }
        "channel" => {
            let (channel, subscribed) = channel(command)?;
            client.session(|tx| tx.set_channel(channel, subscribed))?;
            Value::Null
        }
        _ => return Err(invalid("unsupported transaction command")),
    })
}

/// The schema check's outcome as language packages report it in `status()`.
pub(super) fn schema_json(state: &SchemaState) -> Value {
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
fn read_now(command: &Value) -> Result<u64> {
    command
        .get("now")
        .map(|v| read_counter(v, false))
        .transpose()
        .map(|now| now.unwrap_or(0))
}
fn read_entropy(command: &Value) -> Result<u64> {
    command
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
fn parameters(command: &Value) -> Result<&[Value]> {
    command["parameters"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid("SQL parameters must be array"))
}
fn channel(command: &Value) -> Result<(String, bool)> {
    let channel = text(command, "channel")?.to_string();
    let subscribed = command["subscribed"]
        .as_bool()
        .ok_or_else(|| invalid("subscribed must be bool"))?;
    Ok((channel, subscribed))
}
/// Invocation options sent beside, never inside, an Action's business args.
/// An absent `store` is the default policy.
fn action_options(command: &Value) -> Result<ActionCallOptions> {
    Ok(ActionCallOptions {
        store: match command.get("store") {
            None => ActionStore::All,
            Some(store) => ActionStore::from_wire(store)?,
        },
    })
}
