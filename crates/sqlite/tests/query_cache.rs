//! Query once result snapshots (#158): canonical keys, additive storage,
//! generation fencing and the Rust coordinator, against real SQLite.
use axton_client::query_cache::QueryCacheKey;
use axton_client::*;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};

const PROJECT: &str = "0b4e7a0e-5d6b-4f0e-9b1c-3f7c2a1d9e10";

fn fields() -> Value {
    json!([{"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},
           {"name":"title","type":{"kind":"scalar","name":"string"},"nullable":false}])
}
fn handler() -> Value {
    json!({"kind":"identity","model":"Todo","fields":[{"name":"id","type":{"kind":"scalar","name":"string"}}]})
}
/// `GetTodos` returns Models plus scalar/list/date metadata; `Ping` is a
/// parameterless void Query; `Rename` is a Mutation.
fn schema_value() -> Value {
    json!({"enums":[],
        "models":[{"name":"Todo","version":1,"identity":["id"],"fields":fields()}],
        "resultModels":[{"name":"Todo","version":1,"identity":["id"],"fields":fields(),"enums":[]}],
        "actions":[
            {"name":"GetTodos","version":1,"kind":"query","inputs":[
                {"kind":"value","name":"projectId","type":{"kind":"scalar","name":"uuid"},"nullable":false},
                {"kind":"value","name":"since","type":{"kind":"scalar","name":"dateTime"},"nullable":true},
                {"kind":"value","name":"tags","type":{"kind":"scalar","name":"string"},"nullable":false,"list":true}],
             "outputs":[
                {"name":"todos","kind":"model","model":"Todo","modelReadVersion":1,"cardinality":"list","source":"handlerIdentity","handlerType":handler()},
                {"name":"total","kind":"value","type":{"kind":"scalar","name":"int"},"cardinality":"single","source":"handlerValue"},
                {"name":"asOf","kind":"value","type":{"kind":"scalar","name":"dateTime"},"cardinality":"single","source":"handlerValue"},
                {"name":"next","kind":"value","type":{"kind":"scalar","name":"string"},"cardinality":"optional","source":"handlerValue"}]},
            {"name":"Ping","version":1,"kind":"query","inputs":[],"outputs":[]},
            {"name":"Rename","version":1,"inputs":[{"kind":"model","name":"todo","model":"Todo","operation":"update","cardinality":"single"}],"outputs":[]}]
    })
}
fn schema() -> Schema {
    Schema::from_value(schema_value()).unwrap()
}
/// The same client contract plus one more Query: compatible for the Model
/// tables, a different result-cache contract.
fn extended_schema() -> Schema {
    let mut value = schema_value();
    value["actions"].as_array_mut().unwrap().push(
        json!({"name":"Count","version":1,"kind":"query","inputs":[],"outputs":[
            {"name":"n","kind":"value","type":{"kind":"scalar","name":"int"},"cardinality":"single","source":"handlerValue"}]}),
    );
    Schema::from_value(value).unwrap()
}
fn open(path: &std::path::Path) -> Client<SqliteStore> {
    Client::open(SqliteStore::open(path).unwrap(), schema()).unwrap()
}
fn args() -> Value {
    json!({"projectId":PROJECT,"since":"2026-01-02T03:04:05Z","tags":["a","b"]})
}
fn key(client: &Client<SqliteStore>, args: Value, store: ActionStore) -> QueryCacheKey {
    client
        .query_cache_key("GetTodos", 1, &args, &store)
        .unwrap()
}
fn result() -> Value {
    json!({"todos":[{"id":"a","title":"A"}],"total":1,"asOf":"2026-01-02T03:04:05.000Z","next":null})
}
fn seed(client: &mut Client<SqliteStore>, id: &str) {
    client
        .transaction(|tx| {
            tx.direct(Operation {
                model: "Todo".into(),
                op: OperationKind::Create,
                identity: json!({ "id": id }),
                values: Some(json!({"title":"local"})),
            })
        })
        .unwrap();
}
fn count(client: &mut Client<SqliteStore>) -> u64 {
    client
        .read_sql("SELECT COUNT(*) AS n FROM axton_query_cache", &[])
        .unwrap()[0]["n"]
        .as_u64()
        .unwrap()
}

#[test]
fn equivalent_arguments_and_store_policies_share_one_key() {
    let dir = tempfile::tempdir().unwrap();
    let client = open(&dir.path().join("db"));
    let base = key(&client, args(), ActionStore::All);
    // Key order, UUID case and zone offsets normalize through the Query's
    // own argument validation.
    let reordered = json!({"tags":["a","b"],"since":"2026-01-02T04:04:05+01:00","projectId":PROJECT.to_uppercase()});
    assert_eq!(key(&client, reordered, ActionStore::All), base);
    // Omitted, `true` and an all-true map are the same canonical policy
    // (GetTodos' only eligible output is `todos`).
    assert_eq!(
        key(
            &client,
            args(),
            ActionStore::Outputs([("todos".to_string(), true)].into())
        ),
        base
    );
    assert_eq!(base.name, "GetTodos");
    assert_eq!(base.version, 1);
    assert_eq!(base.store, "true");
    assert_eq!(
        base.args,
        r#"{"projectId":"0b4e7a0e-5d6b-4f0e-9b1c-3f7c2a1d9e10","since":"2026-01-02T03:04:05.000Z","tags":["a","b"]}"#
    );
    assert_eq!(base.contract, client.query_cache_contract());
    assert_eq!(
        base.key.len(),
        64,
        "hex SHA-256 of the canonical components"
    );
}

#[test]
fn different_arguments_store_variants_and_order_get_distinct_keys() {
    let dir = tempfile::tempdir().unwrap();
    let client = open(&dir.path().join("db"));
    let base = key(&client, args(), ActionStore::All).key;
    let mut distinct = vec![base.clone()];
    let mut add = |k: QueryCacheKey| {
        assert!(!distinct.contains(&k.key), "{k:?}");
        distinct.push(k.key);
    };
    let mut reversed = args();
    reversed["tags"] = json!(["b", "a"]);
    add(key(&client, reversed, ActionStore::All));
    let mut null_since = args();
    null_since["since"] = Value::Null;
    add(key(&client, null_since, ActionStore::All));
    add(key(&client, args(), ActionStore::None));
    let none_map = key(
        &client,
        args(),
        ActionStore::Outputs([("todos".to_string(), false)].into()),
    );
    assert_eq!(none_map.store, r#"{"todos":false}"#);
    add(none_map);
}

#[test]
fn keys_are_only_for_queries_and_invalid_input_fails_before_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = open(&dir.path().join("db"));
    let store = ActionStore::All;
    assert!(
        client
            .query_cache_key("Rename", 1, &json!({"todo":{"id":"a","title":"x"}}), &store)
            .is_err(),
        "a Mutation has no once cache"
    );
    assert!(
        client
            .query_cache_key("Missing", 1, &json!({}), &store)
            .is_err()
    );
    assert!(
        client
            .query_cache_key("GetTodos", 2, &args(), &store)
            .is_err()
    );
    let mut undeclared = args();
    undeclared["extra"] = json!(1);
    assert!(
        client
            .query_cache_key("GetTodos", 1, &undeclared, &store)
            .is_err()
    );
    let mut bad_uuid = args();
    bad_uuid["projectId"] = json!("not-a-uuid");
    assert!(
        client
            .query_cache_key("GetTodos", 1, &bad_uuid, &store)
            .is_err()
    );
    assert!(
        client
            .query_cache_key(
                "GetTodos",
                1,
                &args(),
                &ActionStore::Outputs([("total".to_string(), false)].into())
            )
            .is_err(),
        "store may name only explicit Model outputs"
    );
    assert!(
        client
            .invalidate_query_once("Rename", 1, &json!({"todo":{"id":"a","title":"x"}}))
            .is_err()
    );
    assert_eq!(count(&mut client), 0);
}

#[test]
fn the_contract_is_a_fingerprint_of_the_complete_client_schema() {
    let dir = tempfile::tempdir().unwrap();
    let a = open(&dir.path().join("a"));
    let b = Client::open(
        SqliteStore::open(dir.path().join("b")).unwrap(),
        extended_schema(),
    )
    .unwrap();
    let again = open(&dir.path().join("c"));
    assert_eq!(a.query_cache_contract(), again.query_cache_contract());
    assert_ne!(a.query_cache_contract(), b.query_cache_contract());
    assert_ne!(
        key(&a, args(), ActionStore::All).key,
        key(&b, args(), ActionStore::All).key
    );
}

#[test]
fn an_existing_database_gains_the_cache_table_without_touching_its_queue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut client = open(&path);
    seed(&mut client, "a");
    client
        .submit_action("Rename", 1, json!({"todo":{"id":"a","title":"x"}}))
        .unwrap();
    let frozen = client.freeze().unwrap().unwrap();
    drop(client);
    // A database written before this table existed.
    SqliteStore::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE axton_query_cache")
        .unwrap();
    let mut client = open(&path);
    assert_eq!(count(&mut client), 0);
    assert_eq!(client.pending_count().unwrap(), 1);
    assert_eq!(client.freeze().unwrap().unwrap(), frozen);
    assert!(!client.schema_state().rebuilt);
}

#[test]
fn successful_empty_and_void_results_persist_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut client = open(&path);
    let todos = key(&client, args(), ActionStore::All);
    let ping = client
        .query_cache_key("Ping", 1, &json!({}), &ActionStore::All)
        .unwrap();
    assert_eq!(client.query_cache_entry(&todos).unwrap(), None);
    let empty = json!({"todos":[],"total":0,"asOf":"2026-01-02T03:04:05.000Z","next":null});
    assert!(client.save_query_result(&todos, None, &empty).unwrap());
    assert!(client.save_query_result(&ping, None, &Value::Null).unwrap());
    drop(client);
    let mut client = open(&path);
    let entry = client.query_cache_entry(&todos).unwrap().unwrap();
    assert_eq!(entry.result, Some(empty));
    // A cached JSON null is a successful void result, not an empty row.
    let entry = client.query_cache_entry(&ping).unwrap().unwrap();
    assert_eq!(entry.result, Some(Value::Null));
    assert_eq!(
        client
            .read_sql("SELECT COUNT(*) AS n FROM Todo", &[])
            .unwrap()[0]["n"],
        0,
        "saving a snapshot never writes Models"
    );
}

#[test]
fn a_result_that_violates_the_output_contract_is_never_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = open(&dir.path().join("db"));
    let todos = key(&client, args(), ActionStore::All);
    assert!(
        client
            .save_query_result(&todos, None, &json!({"todos":[]}))
            .is_err()
    );
    assert_eq!(count(&mut client), 0);
}

#[test]
fn invalidation_leaves_a_fenced_tombstone_for_every_store_variant() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = open(&dir.path().join("db"));
    let all = key(&client, args(), ActionStore::All);
    let none = key(&client, args(), ActionStore::None);
    let mut other_args = args();
    other_args["tags"] = json!([]);
    let other = key(&client, other_args, ActionStore::All);
    for k in [&all, &none, &other] {
        assert!(client.save_query_result(k, None, &result()).unwrap());
    }
    let before = client.query_cache_entry(&all).unwrap().unwrap().generation;
    // Equivalent arguments name the same argument set.
    let equivalent =
        json!({"tags":["a","b"],"since":"2026-01-02T03:04:05.000+00:00","projectId":PROJECT});
    client
        .invalidate_query_once("GetTodos", 1, &equivalent)
        .unwrap();
    for k in [&all, &none] {
        let entry = client.query_cache_entry(k).unwrap().unwrap();
        assert_eq!(entry.result, None, "tombstone");
        assert_ne!(entry.generation, before);
    }
    assert_eq!(
        client.query_cache_entry(&other).unwrap().unwrap().result,
        Some(result()),
        "another argument set is untouched"
    );
    // A save fenced by the old generation cannot repopulate it.
    assert!(
        !client
            .save_query_result(&all, Some(&before), &result())
            .unwrap()
    );
    assert!(!client.save_query_result(&all, None, &result()).unwrap());
    assert_eq!(
        client.query_cache_entry(&all).unwrap().unwrap().result,
        None
    );
    let current = client.query_cache_entry(&all).unwrap().unwrap().generation;
    assert!(
        client
            .save_query_result(&all, Some(&current), &result())
            .unwrap()
    );
    assert_eq!(
        client.query_cache_entry(&all).unwrap().unwrap().generation,
        current,
        "a successful save keeps the generation"
    );
}

#[test]
fn independent_databases_hold_independent_results() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = open(&dir.path().join("first"));
    let mut second = open(&dir.path().join("second"));
    let k = key(&first, args(), ActionStore::All);
    assert_eq!(k, key(&second, args(), ActionStore::All));
    assert!(first.save_query_result(&k, None, &result()).unwrap());
    assert!(first.query_cache_entry(&k).unwrap().is_some());
    assert_eq!(second.query_cache_entry(&k).unwrap(), None);
}

#[test]
fn a_schema_change_prunes_obsolete_partitions_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut client = open(&path);
    let old = key(&client, args(), ActionStore::All);
    assert!(client.save_query_result(&old, None, &result()).unwrap());
    drop(client);
    // Reopening with the same contract keeps the snapshot.
    let mut client = open(&path);
    assert!(client.query_cache_entry(&old).unwrap().is_some());
    drop(client);
    let mut client = Client::open(SqliteStore::open(&path).unwrap(), extended_schema()).unwrap();
    assert_eq!(count(&mut client), 0, "the old contract's rows are gone");
    let new = key(&client, args(), ActionStore::All);
    assert!(client.save_query_result(&new, None, &result()).unwrap());
    drop(client);
    // Going back is another contract change: nothing is resurrected.
    let mut client = open(&path);
    assert_eq!(count(&mut client), 0);
}

#[test]
fn a_rebuilt_replica_starts_with_an_empty_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let factory = || -> StoreFactory<SqliteStore> { Box::new(|p| SqliteStore::open(p)) };
    let mut client = Client::open_at(&path, schema(), factory(), false).unwrap();
    let k = key(&client, args(), ActionStore::All);
    assert!(client.save_query_result(&k, None, &result()).unwrap());
    drop(client);
    // An incompatible Model change rebuilds beside the old file.
    let mut value = schema_value();
    value["models"][0]["fields"][1]["type"]["name"] = json!("int");
    value["resultModels"][0]["fields"][1]["type"]["name"] = json!("int");
    let mut client =
        Client::open_at(&path, Schema::from_value(value).unwrap(), factory(), false).unwrap();
    assert!(client.schema_state().rebuilt);
    assert_eq!(count(&mut client), 0);
}
