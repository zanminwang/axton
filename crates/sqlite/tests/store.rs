use axton_client::ClientStore;
use axton_sqlite::SqliteStore;
use serde_json::json;

fn store() -> (tempfile::TempDir, SqliteStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("db")).unwrap();
    (dir, store)
}

#[test]
fn reader_sees_only_committed_rows_and_writer_sees_its_own() {
    let (_dir, mut s) = store();
    s.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, flag INTEGER)")
        .unwrap();
    s.begin().unwrap();
    s.execute(
        "INSERT INTO t VALUES (?,?,?)",
        &[json!(1), json!("a"), json!(true)],
    )
    .unwrap();
    assert_eq!(
        s.query("SELECT name, flag FROM t", &[]).unwrap().rows,
        vec![vec![json!("a"), json!(1)]]
    );
    assert!(
        s.query_committed("SELECT name FROM t", &[])
            .unwrap()
            .rows
            .is_empty()
    );
    s.commit().unwrap();
    let rows = s.query_committed("SELECT name, flag FROM t", &[]).unwrap();
    assert_eq!(rows.columns, vec!["name", "flag"]);
    assert_eq!(rows.rows, vec![vec![json!("a"), json!(1)]]);
}

#[test]
fn savepoints_nest_and_rollback_independently() {
    let (_dir, mut s) = store();
    s.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .unwrap();
    s.begin().unwrap();
    s.execute("INSERT INTO t VALUES (1)", &[]).unwrap();
    s.savepoint("a").unwrap();
    s.execute("INSERT INTO t VALUES (2)", &[]).unwrap();
    s.rollback_to("a").unwrap();
    s.savepoint("b").unwrap();
    s.execute("INSERT INTO t VALUES (3)", &[]).unwrap();
    s.release("b").unwrap();
    s.commit().unwrap();
    let ids = s
        .query_committed("SELECT id FROM t ORDER BY id", &[])
        .unwrap()
        .rows;
    assert_eq!(ids, vec![vec![json!(1)], vec![json!(3)]]);
    s.begin().unwrap();
    s.execute("INSERT INTO t VALUES (4)", &[]).unwrap();
    s.rollback().unwrap();
    assert_eq!(
        s.query_committed("SELECT COUNT(*) FROM t", &[])
            .unwrap()
            .rows,
        vec![vec![json!(2)]]
    );
}

#[test]
fn queries_refuse_writes_and_arrays_travel_as_json_text() {
    let (_dir, mut s) = store();
    s.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, tags TEXT)")
        .unwrap();
    s.execute("INSERT INTO t VALUES (?,?)", &[json!(1), json!(["x", "y"])])
        .unwrap();
    assert_eq!(
        s.query_committed("SELECT tags FROM t", &[]).unwrap().rows,
        vec![vec![json!("[\"x\",\"y\"]")]]
    );
    assert!(
        s.query_committed("DELETE FROM t RETURNING id", &[])
            .is_err()
    );
    assert!(s.query("PRAGMA user_version=10", &[]).is_err());
    assert!(
        s.query_committed("INSERT INTO t VALUES (2, NULL)", &[])
            .is_err()
    );
}

#[test]
fn second_writer_waits_then_fails_on_conflicting_immediate_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut a = SqliteStore::open(&path).unwrap();
    let mut b = SqliteStore::open(&path).unwrap();
    a.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .unwrap();
    a.begin().unwrap();
    assert!(
        b.begin().is_err(),
        "busy timeout must expire into an error, not a hang"
    );
    a.rollback().unwrap();
    b.begin().unwrap();
    b.rollback().unwrap();
}
