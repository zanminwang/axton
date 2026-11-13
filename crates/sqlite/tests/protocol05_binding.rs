use axton_client::{Client, Schema};
use axton_sqlite::SqliteStore;

fn locks() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        SqliteStore::set_application_data_directory(
            std::env::temp_dir().join("axton-task8-binding-locks"),
        )
        .unwrap()
    });
}
fn schema() -> Schema {
    Schema::from_value(
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap(),
    )
    .unwrap()
}
#[test]
fn reopen_keeps_context_and_rejects_stream_rebinding_before_schema_changes() {
    locks();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut client = Client::open05(
        SqliteStore::open_exclusive05(&path, "User:alice").unwrap(),
        schema(),
        "User:alice",
    )
    .unwrap();
    let context = client.request_context05().unwrap().clone();
    drop(client);
    let failed = SqliteStore::open_exclusive05(&path, "User:bob");
    assert!(
        failed
            .err()
            .unwrap()
            .to_string()
            .contains("Stream mismatch")
    );
    let mut reopened = Client::open05(
        SqliteStore::open_exclusive05(&path, "User:alice").unwrap(),
        schema(),
        "User:alice",
    )
    .unwrap();
    assert_eq!(reopened.request_context05().unwrap(), context);
}
#[test]
fn exclusive_store_rejects_another_open_of_the_same_physical_file() {
    locks();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let first = SqliteStore::open_exclusive05(&path, "User:a").unwrap();
    assert!(
        SqliteStore::open_exclusive05(&path, "User:a")
            .err()
            .unwrap()
            .to_string()
            .contains("store_in_use")
    );
    #[cfg(unix)]
    {
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(
            SqliteStore::open_exclusive05(&alias, "User:a")
                .err()
                .unwrap()
                .to_string()
                .contains("store_in_use")
        );
        let hard = dir.path().join("hard");
        std::fs::hard_link(&path, &hard).unwrap();
        assert!(
            SqliteStore::open_exclusive05(&hard, "User:a")
                .err()
                .unwrap()
                .to_string()
                .contains("store_in_use")
        );
    }
    drop(first);
    SqliteStore::open_exclusive05(&path, "User:a").unwrap();
}
#[test]
fn exclusive_lock_is_held_across_processes() {
    locks();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let _first = SqliteStore::open_exclusive05(&path, "User:a").unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "exclusive_lock_child",
            "--nocapture",
        ])
        .env("AXTON_LOCK_TEST_PATH", &path)
        .status()
        .unwrap();
    assert!(status.success());
}
/// Executed as a separate process by exclusive_lock_is_held_across_processes.
#[test]
#[ignore]
fn exclusive_lock_child() {
    locks();
    let path = std::env::var_os("AXTON_LOCK_TEST_PATH").expect("parent supplies database path");
    assert!(
        SqliteStore::open_exclusive05(path, "User:a")
            .err()
            .unwrap()
            .to_string()
            .contains("store_in_use")
    );
}

#[cfg(unix)]
#[test]
fn exclusive_owner_close_releases_lock_while_preexec_child_retains_descriptors() {
    unsafe extern "C" {
        fn fork() -> i32;
        fn pipe(fds: *mut i32) -> i32;
        fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
        fn write(fd: i32, buf: *const u8, count: usize) -> isize;
        fn close(fd: i32) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        fn _exit(status: i32) -> !;
    }
    locks();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let first = SqliteStore::open_exclusive05(&path, "User:a").unwrap();
    let mut ready = [0; 2];
    let mut release = [0; 2];
    assert_eq!(unsafe { pipe(ready.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { pipe(release.as_mut_ptr()) }, 0);
    let child = unsafe { fork() };
    assert!(child >= 0);
    if child == 0 {
        // The harness is multithreaded: no Rust allocation, SQLite, panic or
        // destructor is allowed after fork. Hold inherited descriptors until
        // the parent has closed its actual Store and attempted to reopen it.
        unsafe {
            close(ready[0]);
            close(release[1]);
            let byte = 1_u8;
            let sent = write(ready[1], &byte, 1);
            let mut command = 0_u8;
            let received = read(release[0], &mut command, 1);
            close(ready[1]);
            close(release[0]);
            _exit(if sent == 1 && received == 1 { 0 } else { 1 });
        }
    }
    unsafe {
        close(ready[1]);
        close(release[0]);
    }
    let mut byte = 0_u8;
    let child_ready = unsafe { read(ready[0], &mut byte, 1) };
    drop(first);
    let reopened = SqliteStore::open_exclusive05(&path, "User:a");

    // Always release/reap before asserting the reopen outcome, including the
    // red case. A test failure must not leave the inherited child lock alive.
    let released = unsafe { write(release[1], &byte, 1) };
    unsafe {
        close(ready[0]);
        close(release[1]);
    }
    let mut status = 0;
    let reaped = unsafe { waitpid(child, &mut status, 0) };
    assert_eq!(child_ready, 1);
    assert_eq!(released, 1);
    assert_eq!(reaped, child);
    assert_eq!(status, 0);
    let second = reopened.expect("closed owner must release ownership before child exec/exit");
    assert!(
        SqliteStore::open_exclusive05(&path, "User:a")
            .err()
            .unwrap()
            .to_string()
            .contains("store_in_use"),
        "explicit owner release must preserve exclusivity of the new Store"
    );
    drop(second);
    SqliteStore::open_exclusive05(&path, "User:a").unwrap();
}
