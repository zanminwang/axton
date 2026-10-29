use axton_client::ClientStore;
use axton_sqlite::SqliteStore;
use serde_json::json;

fn store() -> (tempfile::TempDir, SqliteStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("db")).unwrap();
    (dir, store)
}

#[test]
fn repeated_reads_follow_schema_changes_on_both_connections_including_empty_results() {
    let (_dir, mut s) = store();
    s.execute_batch("CREATE TABLE Entry(id TEXT PRIMARY KEY); INSERT INTO Entry VALUES('a')")
        .unwrap();
    for sql in ["SELECT * FROM Entry", "SELECT * FROM Entry WHERE 0"] {
        assert_eq!(s.query(sql, &[]).unwrap().columns, ["id"]);
        assert_eq!(s.query_committed(sql, &[]).unwrap().columns, ["id"]);
    }
    s.execute("ALTER TABLE Entry ADD COLUMN text TEXT DEFAULT 'new'", &[])
        .unwrap();
    for sql in ["SELECT * FROM Entry", "SELECT * FROM Entry WHERE 0"] {
        for result in [
            s.query(sql, &[]).unwrap(),
            s.query_committed(sql, &[]).unwrap(),
        ] {
            assert_eq!(result.columns, ["id", "text"]);
            if sql.ends_with('0') {
                assert!(result.rows.is_empty());
            } else {
                assert_eq!(result.rows, vec![vec![json!("a"), json!("new")]]);
            }
        }
    }
}

#[test]
fn repeated_statements_reset_bindings_and_recover_after_constraint_failure_and_rollback() {
    let (_dir, mut s) = store();
    s.execute_batch("CREATE TABLE Entry(id TEXT PRIMARY KEY, text TEXT NOT NULL)")
        .unwrap();
    let insert = "INSERT INTO Entry VALUES(?,?)";
    s.execute(insert, &[json!("a"), json!("first")]).unwrap();
    s.begin().unwrap();
    assert!(
        s.execute(insert, &[json!("a"), json!("duplicate")])
            .is_err()
    );
    s.execute(insert, &[json!("b"), json!("rolled back")])
        .unwrap();
    s.rollback().unwrap();
    assert!(s.execute(insert, &[json!("c")]).is_err());
    s.execute(insert, &[json!("c"), json!("last")]).unwrap();
    for (id, expected) in [("a", "first"), ("c", "last")] {
        assert_eq!(
            s.query("SELECT text FROM Entry WHERE id=?", &[json!(id)])
                .unwrap()
                .rows,
            vec![vec![json!(expected)]]
        );
    }
    assert!(
        s.query("SELECT text FROM Entry WHERE id=?", &[json!("b")])
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn warmed_reads_still_authorize_watch_dependencies_and_refuse_writes() {
    let (_dir, mut s) = store();
    s.execute_batch("CREATE TABLE Entry(id TEXT); CREATE TABLE axton_hidden(id TEXT)")
        .unwrap();
    for sql in [
        "SELECT count(*) AS n FROM Entry",
        "SELECT count(*) AS n FROM axton_hidden",
    ] {
        s.query_committed(sql, &[]).unwrap();
    }
    assert_eq!(
        s.read_tables("SELECT count(*) AS n FROM Entry")
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["Entry"]
    );
    assert!(
        s.read_tables("SELECT count(*) AS n FROM axton_hidden")
            .is_err()
    );
    assert_eq!(
        s.read_tables("SELECT count(*) AS n FROM Entry")
            .unwrap()
            .len(),
        1
    );
    for _ in 0..2 {
        assert!(s.query("DELETE FROM Entry RETURNING id", &[]).is_err());
        assert!(
            s.query_committed("DELETE FROM Entry RETURNING id", &[])
                .is_err()
        );
    }
    s.execute("INSERT INTO Entry VALUES(?)", &[json!("a")])
        .unwrap();
    assert_eq!(
        s.query_committed("SELECT count(*) AS n FROM Entry", &[])
            .unwrap()
            .rows,
        vec![vec![json!(1)]]
    );
}
