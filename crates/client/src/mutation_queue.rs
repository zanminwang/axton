//! Input is decomposed once. Frozen requests are rebuilt from retained entries
//! and descriptor history; defaults are never regenerated on retry.
use crate::{
    engine::{Engine, as_u64},
    queue::OpKind,
    *,
};
use axton_core::{ActionInputDescriptor, canonical_json, normalize_action_args};
use axton_protocols::wire::{self, Validate};
use serde_json::{Value, json};
use std::collections::BTreeSet;
pub(crate) fn text<T: serde::Serialize>(v: &T) -> Result<Value> {
    Ok(json!(canonical_json(&serde_json::to_value(v)?)?))
}
pub(crate) fn decode<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T> {
    Ok(serde_json::from_str(
        v.as_str().ok_or_else(|| invalid("saved JSON missing"))?,
    )?)
}
fn kind(k: OpKind) -> &'static str {
    match k {
        OpKind::Wire => "wire",
        OpKind::Effect => "effect",
    }
}
fn operation(k: OperationKind) -> wire::Operation {
    match k {
        OperationKind::Create => wire::Operation::Create,
        OperationKind::Update => wire::Operation::Update,
        OperationKind::Delete => wire::Operation::Delete,
    }
}
impl<S: ClientStore> Engine<'_, S> {
    pub(crate) fn retained_schema05(&mut self, descriptor: &str) -> Result<Schema> {
        decode(
            &self
                .scalar(
                    "SELECT descriptor FROM axton_descriptor WHERE context=?",
                    &[json!(descriptor)],
                )?
                .ok_or_else(|| invalid("descriptor missing"))?,
        )
    }
    /// A queued Mutation's piece on `model` is used only when that Model's
    /// shape in the Mutation's retained descriptor needs no reset under the
    /// current schema. A Model the current schema lacks is not shared.
    pub(crate) fn shared05(&mut self, descriptor: &str, model: &str) -> Result<bool> {
        let key = (descriptor.to_owned(), model.to_owned());
        if let Some(shared) = self.shared.models().get(&key) {
            return Ok(shared);
        }
        let retained = self.retained_schema05(descriptor)?;
        let shared = !matches!(
            Schema::model_compatibility(&retained, self.schema, model),
            axton_core::Compatibility::Incompatible(_)
        );
        self.shared.models().insert(key, shared);
        Ok(shared)
    }
    /// The descriptor context a queued Mutation was enqueued under.
    pub(crate) fn mutation_descriptor05(&mut self, id: u64) -> Result<String> {
        self.scalar(
            "SELECT descriptor FROM axton_mutation_queue WHERE id=?",
            &[json!(id)],
        )?
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| invalid("queued Mutation missing"))
    }
    pub(crate) fn wire_ops05(&mut self, id: u64) -> Result<Vec<wire::MutationOperation>> {
        self.rows("SELECT step,input_path,operation,model,identity,value FROM axton_mutation_queue_operation WHERE mutation_id=? AND input_path IS NOT NULL ORDER BY step",&[json!(id)])?.rows.into_iter().map(|r|Ok(wire::MutationOperation{step:as_u64(&r[0])?,input_path:r[1].as_str().ok_or_else(||invalid("input path missing"))?.into(),operation:serde_json::from_value(json!(r[2]))?,model:r[3].as_str().map(str::to_owned),identity:if r[4].is_null(){Value::Null}else{decode(&r[4])?},value:if r[5].is_null(){Value::Null}else{decode(&r[5])?}})).collect()
    }
    pub(crate) fn input05(&mut self, id: u64) -> Result<Value> {
        wire::reconstruct_input(&self.wire_ops05(id)?)
    }
    pub(crate) fn insert_owned05(
        &mut self,
        id: u64,
        step: u64,
        k: OpKind,
        path: Option<&str>,
        op: &Operation,
    ) -> Result<()> {
        let key = self.schema.record_key(&op.model, &op.identity)?;
        let history = self.evidence05(&key)?.history;
        let seq = self.allocate05("next_local_sequence")?;
        self.exec("axton_mutation_queue_operation","INSERT INTO axton_mutation_queue_operation(mutation_id,step,local_sequence,input_path,kind,model,identity,operation,value,owner_history) VALUES(?,?,?,?,?,?,?,?,?,?)",&[json!(id),json!(step),json!(seq),path.map_or(Value::Null,|v|json!(v)),json!(kind(k)),json!(op.model),text(&op.identity)?,serde_json::to_value(operation(op.op))?,op.values.as_ref().map(text).transpose()?.unwrap_or(Value::Null),text(&history)?])?;
        Ok(())
    }
    pub(crate) fn enqueue05(
        &mut self,
        name: &str,
        version: u64,
        mut input: Value,
    ) -> Result<SubmittedCall> {
        let context = self.context05()?;
        let descriptor = self.schema.action(name, version)?.clone();
        if descriptor.kind != CallKind::Mutation {
            return Err(invalid("only Mutations can be queued"));
        }
        let present = input
            .as_object()
            .ok_or_else(|| invalid("input must be object"))?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        crate::defaults::fill_action_args(self.schema, &descriptor, &mut input);
        let mut input = normalize_action_args(self.schema, &descriptor, &input)?;
        axton_core::validate_action_bindings(self.schema, &descriptor, &input)?;
        for slot in &descriptor.inputs {
            if let ActionInputDescriptor::Model {
                name, cardinality, ..
            } = slot
                && cardinality == "optional"
                && !present.contains(name)
            {
                input.as_object_mut().unwrap().remove(name);
            }
        }
        let mut mutation = Mutation::new(
            name,
            actions::derive_operations(self.schema, &descriptor, &input)?,
        );
        mutation.version = version;
        mutation.args = Some(input.clone());
        mutation.call_id = Some("local".into());
        // Derive prerequisites/dependencies from the concrete input before insertion.
        crate::policies::derive(self, &mut mutation)?;
        let id = self.allocate_ordinal()?;
        let descriptor_context = self
            .scalar("SELECT schema_descriptor FROM axton_store", &[])?
            .ok_or_else(|| invalid("descriptor missing"))?;
        self.exec("axton_mutation_queue","INSERT INTO axton_mutation_queue(id,name,descriptor_version,descriptor) VALUES(?,?,?,?)",&[json!(id),json!(name),json!(version),descriptor_context])?;
        let mut step = 0;
        let mut wire = mutation.operations.iter();
        for slot in &descriptor.inputs {
            let name = slot.name();
            let Some(value) = input.get(name) else {
                continue;
            };
            let count = match slot {
                ActionInputDescriptor::Model { cardinality, .. } => match cardinality.as_str() {
                    "optional" if value.is_null() => 0,
                    "list" => value
                        .as_array()
                        .ok_or_else(|| invalid("invalid list"))?
                        .len(),
                    _ => 1,
                },
                _ => 0,
            };
            if count == 0 {
                let seq = self.allocate05("next_local_sequence")?;
                self.exec("axton_mutation_queue_operation","INSERT INTO axton_mutation_queue_operation(mutation_id,step,local_sequence,input_path,kind,operation,value,owner_history) VALUES(?,?,?,?,'argument','argument',?,'{}')",&[json!(id),json!(step),json!(seq),json!(name),text(value)?])?;
                step += 1;
                continue;
            }
            for index in 0..count {
                let op = wire
                    .next()
                    .ok_or_else(|| invalid("missing derived input"))?;
                let path = if matches!(slot,ActionInputDescriptor::Model{cardinality,..} if cardinality=="list")
                {
                    format!("{name}[{index}]")
                } else {
                    name.to_owned()
                };
                self.apply_in_order(op, |e, k, op| {
                    e.insert_owned05(
                        id,
                        step,
                        k,
                        if k == OpKind::Wire { Some(&path) } else { None },
                        &op,
                    )?;
                    step += 1;
                    Ok(())
                })?;
            }
        }
        for (k, deps) in [
            ("lifecycle", mutation.lifecycle_dependencies),
            ("sequence", mutation.sequence_dependencies),
        ] {
            for dep in deps {
                self.exec(
                    "axton_mutation_dependency",
                    "INSERT INTO axton_mutation_dependency(ordinal,depends_on,kind) VALUES(?,?,?)",
                    &[json!(id), json!(dep), json!(k)],
                )?;
            }
        }
        for key in mutation.prerequisites {
            self.exec("axton_mutation_prerequisite","INSERT INTO axton_mutation_prerequisite(ordinal,key,error) VALUES(?,?,(SELECT MAX(error) FROM axton_mutation_prerequisite WHERE key=?))",&[json!(id),json!(key),json!(key)])?;
        }
        Ok(SubmittedCall {
            call_id: format!("{}:{id}", context.store_id),
            ordinal: id,
        })
    }
    pub(crate) fn reconstruct_batch05(&mut self, batch: u64) -> Result<wire::MutationRequest> {
        let rows=self.rows("SELECT id,name,descriptor_version,descriptor,batch_digest FROM axton_mutation_queue WHERE batch_id=? ORDER BY id",&[json!(batch)])?.rows;
        if rows.is_empty() {
            return Err(invalid("unknown Batch"));
        }
        // The materialization is transport: a Batch is sent in the Store's
        // current contract, whichever one it was frozen under.
        let context = self.context05()?;
        let mut mutations = vec![];
        for r in &rows {
            let id = as_u64(&r[0])?;
            let name = r[1].as_str().unwrap().to_owned();
            let version = as_u64(&r[2])?;
            let descriptor = r[3].as_str().unwrap().to_owned();
            let operations = self.wire_ops05(id)?;
            let schema = self.retained_schema05(&descriptor)?;
            let action = schema.action(&name, version)?;
            let input = wire::reconstruct_input(&operations)?;
            let normalized = normalize_action_args(&schema, action, &input)?;
            for (key, value) in input.as_object().unwrap() {
                if normalized.get(key) != Some(value) {
                    return Err(invalid("noncanonical saved input"));
                }
            }
            axton_core::validate_action_bindings(&schema, action, &normalized)?;
            // Ensure paths encode exactly the declared slot/model/operation kinds.
            let expected = actions::derive_operations(&schema, action, &normalized)?;
            let actual: Vec<_> = operations
                .iter()
                .filter(|o| o.model.is_some())
                .map(|o| Operation {
                    model: o.model.clone().unwrap(),
                    identity: o.identity.clone(),
                    op: match o.operation {
                        wire::Operation::Create => OperationKind::Create,
                        wire::Operation::Update => OperationKind::Update,
                        _ => OperationKind::Delete,
                    },
                    values: if o.value.is_null() {
                        None
                    } else {
                        Some(o.value.clone())
                    },
                })
                .collect();
            if serde_json::to_value(expected)? != serde_json::to_value(actual)? {
                return Err(invalid("input operations differ from retained descriptor"));
            }
            for o in &operations {
                let slot = o.input_path.split('[').next().unwrap();
                let declared = action
                    .inputs
                    .iter()
                    .find(|s| s.name() == slot)
                    .ok_or_else(|| invalid("unknown slot"))?;
                if let ActionInputDescriptor::Model {
                    model,
                    operation,
                    cardinality,
                    ..
                } = declared
                {
                    if o.model.is_some()
                        && (o.model.as_deref() != Some(model)
                            || serde_json::to_value(&o.operation)? != operation.as_str())
                    {
                        return Err(invalid("wrong Model slot"));
                    }
                    if o.operation == wire::Operation::Argument
                        && !((cardinality == "optional" && o.value.is_null())
                            || (cardinality == "list" && o.value == json!([])))
                    {
                        return Err(invalid("invalid Model argument sentinel"));
                    }
                } else if o.operation != wire::Operation::Argument {
                    return Err(invalid("Value slot is not argument"));
                }
            }
            mutations.push(wire::Mutation {
                id,
                name,
                version,
                descriptor: wire::mutation_descriptor_digest(action)?,
                operations,
            });
        }
        let mut request = wire::MutationRequest {
            context,
            batch_id: batch,
            digest: "0".repeat(64),
            mutations,
        };
        request.digest = wire::batch_digest(&request)?;
        request.validate()?;
        if rows
            .iter()
            .any(|r| !r[4].is_null() && r[4] != request.digest)
        {
            // Includes Batches frozen under an earlier digest definition.
            return Err(axton_core::Error::Terminal(format!(
                "frozen Batch {batch} no longer matches its saved digest and cannot be resent; reset the Store to discard it"
            )));
        }
        Ok(request)
    }
    pub(crate) fn freeze05(&mut self) -> Result<Option<wire::MutationRequest>> {
        // Push waits for the handshake: a Mutation then executes after the
        // head read, so its syncCursor is at or after the Store's start S.
        let store = self
            .rows(
                "SELECT last_acknowledged_batch_id,start_cursor FROM axton_store",
                &[],
            )?
            .rows
            .remove(0);
        if store[1].is_null() {
            return Ok(None);
        }
        let last = as_u64(&store[0])?;
        if let Some(batch) = self
            .scalar(
                "SELECT MIN(batch_id) FROM axton_mutation_queue WHERE batch_id>?",
                &[json!(last)],
            )?
            .filter(|v| !v.is_null())
        {
            return self.reconstruct_batch05(as_u64(&batch)?).map(Some);
        }
        // Beginning the next Batch prunes the server's previous outcomes.
        // Keep that ownership window until every assigned member settles locally.
        if self.scalar(
            "SELECT 1 FROM axton_mutation_queue WHERE batch_id IS NOT NULL AND reconciled=0 LIMIT 1",
            &[],
        )?.is_some() {
            return Ok(None);
        }
        // A terminal refusal finishes sequence ordering. Lifecycle edges
        // still refuse their dependents; accepted predecessors wait for settlement.
        let rows=self.rows("SELECT id FROM axton_mutation_queue q WHERE batch_id IS NULL AND reconciled=0 AND rejection_code IS NULL AND NOT EXISTS(SELECT 1 FROM axton_mutation_prerequisite p WHERE p.ordinal=q.id) AND NOT EXISTS(SELECT 1 FROM axton_mutation_dependency d JOIN axton_mutation_queue p ON p.id=d.depends_on WHERE d.ordinal=q.id AND (p.reconciled=0 OR (d.kind='lifecycle' AND p.rejection_code IS NOT NULL))) ORDER BY id",&[])?.rows;
        if rows.is_empty() {
            return Ok(None);
        }
        let batch = last
            .checked_add(1)
            .filter(|v| *v <= axton_core::MAX_SAFE_INTEGER)
            .ok_or_else(|| invalid("Batch counter exhausted"))?;
        for r in rows {
            self.exec(
                "axton_mutation_queue",
                "UPDATE axton_mutation_queue SET batch_id=? WHERE id=?",
                &[json!(batch), r[0].clone()],
            )?;
        }
        let request = self.reconstruct_batch05(batch)?;
        self.exec(
            "axton_mutation_queue",
            "UPDATE axton_mutation_queue SET batch_digest=? WHERE batch_id=?",
            &[json!(request.digest), json!(batch)],
        )?;
        Ok(Some(request))
    }
}
impl<S: ClientStore> Client<S> {
    pub fn freeze_batch05(&mut self) -> Result<Option<wire::MutationRequest>> {
        self.write(|e| e.freeze05())
    }
}
impl<S: ClientStore> ClientTransaction<'_, S> {
    pub fn submit_mutation05(
        &mut self,
        name: &str,
        version: u64,
        input: Value,
    ) -> Result<SubmittedCall> {
        self.savepoint(|tx| tx.engine.enqueue05(name, version, input))
    }
}
