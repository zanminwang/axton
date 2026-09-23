#![allow(dead_code)]
//! Helpers shared by every client-facing integration test in this crate.
use axton_client::*;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn schema() -> Schema {
    Schema::from_value(
        serde_json::from_str(include_str!("../../../../fixtures/schemas/entry.json")).unwrap(),
    )
    .unwrap()
}
pub fn open(path: &std::path::Path) -> Client<SqliteStore> {
    Client::open(SqliteStore::open(path).unwrap(), schema()).unwrap()
}
pub fn key() -> RecordKey {
    schema().record_key("Entry", &json!({"id":"e"})).unwrap()
}
pub fn update(text: &str) -> Operation {
    Operation {
        model: "Entry".into(),
        op: OperationKind::Update,
        identity: json!({"id":"e"}),
        values: Some(json!({ "text": text })),
    }
}
pub fn mutation(text: &str) -> Mutation {
    Mutation::new("Edit", vec![update(text)])
}
/// A one-channel page moving `channel` from `from` to `to` (its head) with
/// `Entry e` at stamp `to`.
pub fn page(channel: &str, from: u64, to: u64, text: Option<&str>) -> PullPage {
    PullPage {
        cursors: BTreeMap::from([(channel.to_string(), CursorRange { from, to, head: to })]),
        changes: vec![authority(text, to)],
    }
}
/// A page for several channels at once, each `(channel, from, to, head)`, with `changes`.
pub fn multi(channels: &[(&str, u64, u64, u64)], changes: Vec<AuthorityRecord>) -> PullPage {
    PullPage {
        cursors: channels
            .iter()
            .map(|(c, from, to, head)| {
                (
                    c.to_string(),
                    CursorRange {
                        from: *from,
                        to: *to,
                        head: *head,
                    },
                )
            })
            .collect(),
        changes,
    }
}
/// The authority of `Entry e` at `stamp`: a state, or `None` for a deletion.
pub fn authority(text: Option<&str>, stamp: u64) -> AuthorityRecord {
    AuthorityRecord {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
        stamp,
        state: text
            .map(|t| json!({"text":t,"note":null}))
            .unwrap_or(Value::Null),
        error: None,
    }
}
/// The authority of any `Entry` at `stamp`.
pub fn authority_of(id: &str, text: Option<&str>, stamp: u64) -> AuthorityRecord {
    let mut record = authority(text, stamp);
    record.identity = json!({ "id": id });
    record
}
/// A receipt answering this client's batch `sequence` with `records` and no rejections.
pub fn receipt(
    c: &mut Client<SqliteStore>,
    sequence: u64,
    records: Vec<AuthorityRecord>,
) -> PushReceipt {
    PushReceipt {
        client_id: c.client_id().to_string(),
        batch_sequence: sequence,
        rejections: vec![],
        records,
    }
}
/// A receipt rejecting `ordinals` with `code` and returning `records` for the rest.
pub fn rejecting(
    c: &mut Client<SqliteStore>,
    sequence: u64,
    ordinals: &[u64],
    code: &str,
    records: Vec<AuthorityRecord>,
) -> PushReceipt {
    let mut r = receipt(c, sequence, records);
    r.rejections = ordinals
        .iter()
        .map(|o| Rejection {
            ordinal: *o,
            code: code.into(),
        })
        .collect();
    r
}
/// Only a subscribed channel may be pulled: `apply_page` drops a page for any other.
pub fn subscribe(c: &mut Client<SqliteStore>, channel: &str) {
    c.transaction(|tx| tx.set_channel(channel.into(), true))
        .unwrap();
}
pub fn seed(c: &mut Client<SqliteStore>, text: &str) {
    c.transaction(|tx| {
        tx.direct(Operation {
            model: "Entry".into(),
            op: OperationKind::Create,
            identity: json!({"id":"e"}),
            values: Some(json!({"text":text,"note":null})),
        })
    })
    .unwrap();
}
pub fn family_schema() -> Schema {
    Schema::from_value(json!({"enums":[],"models":[
 {"name":"Book","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"title","nullable":false,"type":{"kind":"scalar","name":"string"}}]},
 {"name":"Comment","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"bookId","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"text","nullable":false,"type":{"kind":"scalar","name":"string"}}],"relations":[{"name":"book","target":"Book","fields":["bookId"],"targetFields":["id"],"onDelete":"delete"}],"unique":[["bookId","text"]]}
]})).unwrap()
}
pub fn create(model: &str, id: &str, values: Value) -> Operation {
    Operation {
        model: model.into(),
        op: OperationKind::Create,
        identity: json!({ "id": id }),
        values: Some(values),
    }
}
pub fn table_count(c: &mut Client<SqliteStore>, table: &str) -> u64 {
    c.read_sql(&format!("SELECT COUNT(*) AS n FROM \"{table}\""), &[])
        .unwrap()[0]["n"]
        .as_u64()
        .unwrap()
}
