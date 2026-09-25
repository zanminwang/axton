//! Durable subscription identity: a subscription is a row with its own
//! allocated ID, and its cursor pair is NULL until a first boundary is
//! committed ([#150](https://github.com/zanminwang/axton/issues/150)).
mod common;
use axton_client::*;
use axton_sqlite::SqliteStore;
use common::*;
use serde_json::json;

/// Run `sql` on its own connection to the same file: the tests reach past the
/// Client to prove the table, not the code writing it, holds the contract.
fn sql(path: &std::path::Path, statement: &str) -> Result<()> {
    let mut store = SqliteStore::open(path)?;
    store.execute_batch(statement)
}

fn state(c: &mut Client<SqliteStore>, scope: &str) -> SubscriptionState {
    c.subscription_state(scope).unwrap().expect("a row")
}

/// The brief's contract: one identity per Scope name, allocated once, kept
/// across duplicate registration, replaced by recreation, and unaffected by an
/// unsubscribe naming the identity that is gone.
#[test]
fn duplicate_registration_keeps_one_identity_and_recreation_allocates_another() -> Result<()> {
    let dir = tempfile::tempdir().unwrap();
    let mut client = open(&dir.path().join("db"));
    let a = client.ensure_subscription("project:123")?;
    let b = client.ensure_subscription("project:123")?;
    assert_eq!(a.subscription_id, b.subscription_id);
    assert_eq!(a.starting_cursor, None);
    assert_eq!(a.cursor, None);
    client.remove_subscription("project:123", a.subscription_id)?;
    let c = client.ensure_subscription("project:123")?;
    assert_ne!(a.subscription_id, c.subscription_id);
    client.remove_subscription("project:123", a.subscription_id)?;
    assert!(client.subscription_state("project:123")?.is_some());
    Ok(())
}

/// Registration is durable and the allocator never recycles: the reopened
/// replica answers with the same identity, and the next Scope gets a new one.
#[test]
fn identity_and_uninitialized_cursors_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    let first = c.ensure_subscription("a").unwrap();
    c.remove_subscription("a", first.subscription_id).unwrap();
    let second = c.ensure_subscription("a").unwrap();
    drop(c);

    let mut c = open(&path);
    let reopened = state(&mut c, "a");
    assert_eq!(reopened.scope, "a");
    assert_eq!(reopened.subscription_id, second.subscription_id);
    assert_eq!((reopened.starting_cursor, reopened.cursor), (None, None));
    assert_eq!(
        c.ensure_subscription("b").unwrap().subscription_id,
        second.subscription_id + 1,
        "the allocator carries on across the reopen"
    );
}

/// A rolled-back transaction registers nothing: no row, no allocation the
/// next subscribe could skip, no subscription generation change.
#[test]
fn a_rolled_back_registration_leaves_no_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    let generation = c.subscription_generation();
    let failed: Result<()> = c.transaction(|tx| {
        tx.set_channel("a".into(), true)?;
        Err(invalid("host failure"))
    });
    assert!(failed.is_err());
    assert!(c.subscription_state("a").unwrap().is_none());
    assert_eq!(c.subscription_generation(), generation);
    assert_eq!(
        c.ensure_subscription("b").unwrap().subscription_id,
        1,
        "the rolled-back allocation is not spent"
    );
}

/// The transaction-scoped path initializes a new row at zero (removed by the
/// first-initialization change) and repeating it changes nothing at all.
#[test]
fn set_channel_initializes_once_and_repeats_without_a_generation_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "a");
    let first = state(&mut c, "a");
    assert_eq!((first.starting_cursor, first.cursor), (Some(0), Some(0)));
    c.apply_page(page("a", 0, 2, Some("A"))).unwrap();
    let moved = state(&mut c, "a");
    assert_eq!(
        (moved.starting_cursor, moved.cursor),
        (Some(0), Some(2)),
        "page application advances the cursor and leaves the origin"
    );
    let generation = c.subscription_generation();
    subscribe(&mut c, "a");
    let again = state(&mut c, "a");
    assert_eq!(
        (again.subscription_id, again.starting_cursor, again.cursor),
        (moved.subscription_id, Some(0), Some(2)),
        "a repeated subscribe reads the row and touches no cursor"
    );
    assert_eq!(
        c.subscription_generation(),
        generation,
        "a repeated subscribe is not a membership change"
    );
}

/// Unsubscribing through the transaction path removes whatever identity the
/// Scope holds, and the next subscribe is a new subscription.
#[test]
fn set_channel_removes_the_current_identity_and_recreation_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "a");
    c.apply_page(page("a", 0, 2, Some("A"))).unwrap();
    let before = state(&mut c, "a");
    let generation = c.subscription_generation();
    c.transaction(|tx| tx.set_channel("a".into(), false))
        .unwrap();
    assert!(c.subscription_state("a").unwrap().is_none());
    assert!(c.subscription_generation() > generation);
    subscribe(&mut c, "a");
    let after = state(&mut c, "a");
    assert_ne!(after.subscription_id, before.subscription_id);
    assert_eq!((after.starting_cursor, after.cursor), (Some(0), Some(0)));
}

/// `remove_subscription` is fenced by identity: an old handle's unsubscribe
/// cannot delete the subscription that replaced it.
#[test]
fn removal_by_a_stale_identity_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    let first = c.ensure_subscription("a").unwrap();
    c.remove_subscription("a", first.subscription_id).unwrap();
    let second = c.ensure_subscription("a").unwrap();
    let generation = c.subscription_generation();
    c.remove_subscription("a", first.subscription_id).unwrap();
    assert_eq!(
        state(&mut c, "a").subscription_id,
        second.subscription_id,
        "the live subscription is untouched"
    );
    assert_eq!(
        c.subscription_generation(),
        generation,
        "nothing changed, so no subscription generation change"
    );
    c.remove_subscription("a", second.subscription_id).unwrap();
    assert!(c.subscription_state("a").unwrap().is_none());
    assert!(c.subscription_generation() > generation);
}

/// An uninitialized row is durable intent without a delivery position: it is
/// in the desired set, out of every pull request, and no page advances it.
#[test]
fn an_uninitialized_subscription_is_never_read_as_cursor_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    c.ensure_subscription("a").unwrap();
    assert_eq!(c.cursor("a").unwrap(), None);
    assert_eq!(c.subscriptions().unwrap(), vec![]);
    assert!(c.desired_channels().unwrap().contains("a"));
    assert_eq!(
        c.downlink_request().unwrap(),
        None,
        "no initialized subscription, no pull"
    );
    c.apply_page(page("a", 0, 1, Some("A"))).unwrap();
    assert_eq!(
        (state(&mut c, "a").starting_cursor, c.cursor("a").unwrap()),
        (None, None),
        "a page cannot initialize a subscription"
    );
    assert!(c.read(&key()).unwrap().is_none());

    subscribe(&mut c, "b");
    c.apply_page(page("b", 0, 1, Some("B"))).unwrap();
    let request = PullRequest::decode(c.downlink_request().unwrap().unwrap().as_bytes()).unwrap();
    assert_eq!(
        request.cursors,
        std::collections::BTreeMap::from([("b".to_string(), 1)]),
        "only initialized subscriptions ask for changes"
    );
}

/// Cursor advancement updates an existing row and never inserts: a page for a
/// Scope this client unsubscribed cannot resurrect it.
#[test]
fn cursor_advancement_cannot_resurrect_a_removed_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "a");
    c.transaction(|tx| tx.set_channel("a".into(), false))
        .unwrap();
    c.apply_page(page("a", 0, 1, Some("A"))).unwrap();
    assert!(c.subscription_state("a").unwrap().is_none());
    assert_eq!(table_count(&mut c, "axton_subscription"), 0);
}

/// The allocator obeys the shared safe-integer bound instead of handing out an
/// identity, or storing a counter, no host can represent.
#[test]
fn an_exhausted_allocator_refuses_a_new_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    drop(c);
    let last = MAX_SAFE_INTEGER - 1;
    sql(
        &path,
        &format!("UPDATE axton_client SET next_subscription = {last}"),
    )
    .unwrap();
    c = open(&path);
    assert_eq!(c.ensure_subscription("a").unwrap().subscription_id, last);
    let error = c.ensure_subscription("b").unwrap_err().to_string();
    assert!(error.contains("exhausted"), "{error}");
    assert!(
        c.subscription_state("b").unwrap().is_none(),
        "the refusal leaves no half-registered row"
    );
    drop(c);
    sql(
        &path,
        &format!("UPDATE axton_client SET next_subscription = {MAX_SAFE_INTEGER}"),
    )
    .unwrap();
    c = open(&path);
    assert!(
        c.ensure_subscription("b").is_err(),
        "the last counter value"
    );
}

/// The stored contract, not the writing code, rejects a half-initialized
/// cursor pair; both fields move together or neither does.
#[test]
fn the_table_refuses_a_half_initialized_cursor_pair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let c = open(&path);
    drop(c);
    for (starting, cursor) in [("0", "NULL"), ("NULL", "0")] {
        let error = sql(
            &path,
            &format!(
                "INSERT INTO axton_subscription (channel, subscription_id, starting_cursor, cursor) VALUES ('a', 1, {starting}, {cursor})"
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("CHECK"), "{error}");
    }
    sql(
        &path,
        "INSERT INTO axton_subscription (channel, subscription_id, starting_cursor, cursor) VALUES ('a', 1, 4, 4)",
    )
    .unwrap();
    let error = sql(
        &path,
        "UPDATE axton_subscription SET cursor = 3 WHERE channel = 'a'",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("CHECK"),
        "a cursor below its origin: {error}"
    );
    let error = sql(
        &path,
        "INSERT INTO axton_subscription (channel, subscription_id, starting_cursor, cursor) VALUES ('b', 1, 0, 0)",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("UNIQUE"), "one identity per row: {error}");
}

/// A subscription's identity is stored, serializable state the host can carry.
#[test]
fn subscription_state_is_serializable() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "a");
    let state = state(&mut c, "a");
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        json!({"scope":"a","subscriptionId":state.subscription_id,"startingCursor":0,"cursor":0})
    );
}
