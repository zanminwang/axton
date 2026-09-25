//! Completion from the receipt ([#55]): a successful response carries the
//! authoritative records the framework read back, so the batch completes on
//! arrival and no channel is awaited. Each test asserts the visible records,
//! the pending work and the retained stamp evidence.
//!
//! [#55]: https://github.com/zanminwang/axton/issues/55
mod common;
use axton_client::*;
use common::*;
use serde_json::json;

/// The record a local create introduces.
fn created() -> RecordKey {
    schema().record_key("Entry", &json!({"id":"n"})).unwrap()
}

fn create_entry() -> Mutation {
    Mutation::new(
        "Create",
        vec![create("Entry", "n", json!({"text":"new","note":null}))],
    )
}

/// Everything complete: no queue, no before image, no rejection.
fn assert_quiet(c: &mut Client<axton_sqlite::SqliteStore>) {
    assert_eq!(c.pending_count().unwrap(), 0, "nothing pending");
    assert_eq!(c.before_image_count().unwrap(), 0, "no base is retained");
    assert!(
        c.rejections().unwrap().is_empty(),
        "the batch was accepted, not rejected"
    );
}

/// The plan's acceptance test: an update completes from its response alone,
/// with the server's normalized value, while the client follows no channel.
#[test]
fn response_completes_without_a_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("hello")).map(|_| ()))
        .unwrap();
    let request: serde_json::Value = serde_json::from_slice(&c.freeze().unwrap().unwrap()).unwrap();
    assert_eq!(
        request["models"],
        json!({"Entry":1}),
        "the read contract is declared"
    );
    let receipt = PushReceipt::decode(
        &serde_json::to_vec(&json!({
            "clientId": request["clientId"], "batchSequence": 1,
            "rejections": [], "records": [{"model":"Entry", "identity":{"id":"e"},
                "stamp":12, "state":{"text":"Hello", "note":null}}]
        }))
        .unwrap(),
    )
    .unwrap();
    let report = c.acknowledge(1, receipt).unwrap();
    assert_eq!((report.applied, report.conflicts()), (1, 0));
    assert_eq!(c.pending_count().unwrap(), 0);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "Hello");
    assert_eq!(c.before_image_count().unwrap(), 0);
    assert_eq!(c.record_stamp(&key()).unwrap(), 12);
    assert_eq!(c.last_completed_push().unwrap(), 1);
    assert!(c.freeze().unwrap().is_none(), "nothing is re-sent");
}

/// Staging order: the authority lands beneath the pending operation and the
/// completed operation is removed afterwards, so the row ends at the server's
/// value, not at the base the operation was replayed over.
#[test]
fn authority_is_staged_under_the_pending_operation_before_it_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("hello")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "hello");
    assert_eq!(c.before_image_count().unwrap(), 1);
    let r = receipt(&mut c, 1, vec![authority(Some("Hello"), 3)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "Hello");
    assert_quiet(&mut c);
}

/// A pending create has no before image; the response's authority is the
/// first base the record ever has and the create survives acceptance.
#[test]
fn accepted_create_survives_with_the_servers_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    c.transaction(|tx| tx.enqueue(create_entry()).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    assert_eq!(c.before_image_count().unwrap(), 0, "a create holds no base");
    let r = receipt(&mut c, 1, vec![authority_of("n", Some("server new"), 1)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&created()).unwrap().unwrap()["text"], "server new");
    assert_eq!(c.record_stamp(&created()).unwrap(), 1);
    assert_quiet(&mut c);
}

/// Authority for a record with no pending operation (a handler's extra change)
/// is written directly and is not disturbed by the final rebuild.
#[test]
fn clean_extra_authority_is_written_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    let r = receipt(
        &mut c,
        1,
        vec![
            authority(Some("B"), 2),
            authority_of("extra", Some("side effect"), 1),
        ],
    );
    c.acknowledge(1, r).unwrap();
    let extra = schema()
        .record_key("Entry", &json!({"id":"extra"}))
        .unwrap();
    assert_eq!(c.read(&extra).unwrap().unwrap()["text"], "side effect");
    assert_eq!(c.record_stamp(&extra).unwrap(), 1);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_quiet(&mut c);
}

/// Channel first: the page delivers the same authority before the response.
/// The response compares equal against the held base, not the optimistic
/// row, so it reports no conflict and still clears the completed operation.
#[test]
fn channel_first_then_receipt_dedups_and_still_completes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    c.apply_page(page("book", 0, 1, Some("A"))).unwrap();
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    let mut delivered = page("book", 1, 2, Some("B"));
    delivered.changes[0].stamp = 7;
    c.apply_page(delivered).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_eq!(
        c.pending_count().unwrap(),
        1,
        "the page never completes a push"
    );
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 7)]);
    let report = c.acknowledge(1, r).unwrap();
    assert_eq!((report.applied, report.conflicts()), (0, 0));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_eq!(c.cursor("book").unwrap(), Some(2));
    assert_quiet(&mut c);
}

/// Receipt first: the later page carries the same stamp and content and
/// rewrites nothing; its cursor still advances.
#[test]
fn receipt_first_then_channel_is_a_no_op_that_advances_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    c.apply_page(page("book", 0, 1, Some("A"))).unwrap();
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 7)]);
    c.acknowledge(1, r).unwrap();
    assert_quiet(&mut c);
    let mut delivered = page("book", 1, 2, Some("B"));
    delivered.changes[0].stamp = 7;
    let report = c.apply_page(delivered).unwrap();
    assert_eq!(
        (report.applied, report.conflicts()),
        (0, 0),
        "the same stamp and content changes nothing"
    );
    assert_eq!(c.cursor("book").unwrap(), Some(2));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
}

/// Newer authority that arrived through a channel before the response is not
/// replaced by the response's older stamp; the operation still completes.
#[test]
fn newer_channel_authority_is_not_regressed_by_an_older_response() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    c.apply_page(page("book", 0, 1, Some("A"))).unwrap();
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    let mut newer = page("book", 1, 2, Some("LATER"));
    newer.changes[0].stamp = 13;
    c.apply_page(newer).unwrap();
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 12)]);
    let report = c.acknowledge(1, r).unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "LATER");
    assert_eq!(c.record_stamp(&key()).unwrap(), 13);
    assert_quiet(&mut c);
}

/// An edit queued after the batch was frozen replays over the response's
/// authority and stays pending; the base is kept for it.
#[test]
fn later_unsent_edit_replays_over_the_returned_authority() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    c.transaction(|tx| tx.enqueue(mutation("C")).map(|_| ()))
        .unwrap();
    let r = receipt(&mut c, 1, vec![authority(Some("SERVER B"), 5)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "C");
    assert_eq!(c.pending_count().unwrap(), 1);
    assert_eq!(c.before_image_count().unwrap(), 1, "the base is kept");
    assert_eq!(
        c.read_sql("SELECT text FROM axton_before_Entry", &[])
            .unwrap(),
        vec![json!({"text":"SERVER B"})],
        "the base is the server's result"
    );
    let status = c.record_status(&key()).unwrap();
    assert_eq!(status["pending"].as_array().unwrap().len(), 1);
    assert_eq!(status["pending"][0]["phase"], "queued");
    c.freeze().unwrap().unwrap();
    let r = receipt(&mut c, 2, vec![authority(Some("SERVER C"), 6)]);
    c.acknowledge(2, r).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "SERVER C");
    assert_quiet(&mut c);
}

/// A record the batch did not touch is left alone, dirty or clean.
#[test]
fn unrelated_records_are_unaffected() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| {
        tx.direct(create("Entry", "other", json!({"text":"O","note":null})))?;
        tx.enqueue(mutation("B"))?;
        Ok(())
    })
    .unwrap();
    c.freeze().unwrap().unwrap();
    let other = schema()
        .record_key("Entry", &json!({"id":"other"}))
        .unwrap();
    c.transaction(|tx| {
        tx.enqueue(Mutation::new(
            "Edit",
            vec![Operation {
                identity: json!({"id":"other"}),
                ..update("O2")
            }],
        ))
        .map(|_| ())
    })
    .unwrap();
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 2)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&other).unwrap().unwrap()["text"], "O2");
    assert_eq!(c.pending_count().unwrap(), 1);
    assert_eq!(c.record_stamp(&other).unwrap(), 0);
}

/// One mutation rejected beside an accepted one: the rejected optimism is
/// removed, the accepted authority lands, and the rejection is retained.
#[test]
fn failed_sibling_mutation_is_rolled_back_beside_the_accepted_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| {
        tx.enqueue(mutation("B"))?;
        tx.enqueue(create_entry())?;
        Ok(())
    })
    .unwrap();
    c.freeze().unwrap().unwrap();
    let r = rejecting(&mut c, 1, &[2], "denied", vec![authority(Some("B"), 2)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert!(
        c.read(&created()).unwrap().is_none(),
        "the rejected create is gone"
    );
    assert_eq!(c.pending_count().unwrap(), 0);
    assert_eq!(c.before_image_count().unwrap(), 0);
    let rejections = c.rejections().unwrap();
    assert_eq!(rejections.len(), 1);
    assert_eq!(
        (rejections[0].ordinal, rejections[0].code.as_str()),
        (2, "denied")
    );
}

/// Companions on records the server reported take the server's authority; a
/// companion on a record it did not report settles into that record's base.
#[test]
fn companions_settle_locally_except_where_the_server_answered() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| {
        tx.direct(create("Entry", "side", json!({"text":"S","note":null})))?;
        let mut m = mutation("B");
        m.companion.push(update("COMPANION"));
        m.companion.push(Operation {
            identity: json!({"id":"side"}),
            ..update("S2")
        });
        tx.enqueue(m)?;
        Ok(())
    })
    .unwrap();
    c.freeze().unwrap().unwrap();
    let side = schema().record_key("Entry", &json!({"id":"side"})).unwrap();
    let r = receipt(&mut c, 1, vec![authority(Some("SERVER"), 2)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(
        c.read(&key()).unwrap().unwrap()["text"],
        "SERVER",
        "the companion on the reported record does not outrank the server"
    );
    assert_eq!(c.read(&side).unwrap().unwrap()["text"], "S2");
    assert_eq!(c.record_stamp(&side).unwrap(), 0, "no stamp is invented");
    assert_quiet(&mut c);
}

/// A rejected mutation's lifecycle dependents are rejected with it.
#[test]
fn rejection_cascades_to_lifecycle_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    c.transaction(|tx| {
        let parent = tx.enqueue(create_entry())?;
        let mut child = Mutation::new(
            "Edit",
            vec![Operation {
                identity: json!({"id":"n"}),
                ..update("edited")
            }],
        );
        child.lifecycle_dependencies.push(parent);
        tx.enqueue(child)?;
        Ok(())
    })
    .unwrap();
    let batch = PushRequest::decode(&c.freeze().unwrap().unwrap()).unwrap();
    assert_eq!(batch.mutations.len(), 1, "the dependent waits");
    let r = rejecting(&mut c, 1, &[1], "denied", vec![]);
    c.acknowledge(1, r).unwrap();
    assert!(c.read(&created()).unwrap().is_none());
    assert_eq!(c.pending_count().unwrap(), 0);
    let codes: Vec<String> = c
        .rejections()
        .unwrap()
        .into_iter()
        .map(|r| r.code)
        .collect();
    assert_eq!(codes, vec!["denied", "dependency.rejected"]);
}

/// Every mutation rejected: no authority, no queue, direct truth preserved.
#[test]
fn all_rejected_batch_removes_optimism_and_keeps_direct_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| {
        tx.enqueue(mutation("B"))?;
        tx.direct(Operation {
            values: Some(json!({"note":"local"})),
            ..update("unused")
        })
    })
    .unwrap();
    c.freeze().unwrap().unwrap();
    let r = rejecting(&mut c, 1, &[1], "denied", vec![]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(
        c.read(&key()).unwrap().unwrap(),
        json!({"id":"e","text":"A","note":"local"})
    );
    assert_eq!(c.pending_count().unwrap(), 0);
    assert_eq!(c.last_completed_push().unwrap(), 1);
    assert!(c.freeze().unwrap().is_none());
}

/// A deletion comes back as null state: the row goes, the stamp stays as the
/// evidence that keeps stale content from resurrecting it.
#[test]
fn deletion_authority_removes_the_row_and_retains_the_stamp() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    c.apply_page(page("book", 0, 1, Some("A"))).unwrap();
    c.transaction(|tx| {
        tx.enqueue(Mutation::new(
            "Delete",
            vec![Operation {
                model: "Entry".into(),
                op: OperationKind::Delete,
                identity: json!({"id":"e"}),
                values: None,
            }],
        ))
        .map(|_| ())
    })
    .unwrap();
    c.freeze().unwrap().unwrap();
    let r = receipt(&mut c, 1, vec![authority(None, 4)]);
    c.acknowledge(1, r).unwrap();
    assert!(c.read(&key()).unwrap().is_none());
    assert_eq!(c.record_stamp(&key()).unwrap(), 4);
    assert_quiet(&mut c);
    let mut stale = page("book", 1, 2, Some("resurrected"));
    stale.changes[0].stamp = 3;
    c.apply_page(stale).unwrap();
    assert!(
        c.read(&key()).unwrap().is_none(),
        "older content cannot resurrect it"
    );
}

/// A receipt that omits an accepted record, names another client or batch, or
/// rejects an ordinal outside the batch is refused whole: the frozen batch
/// stays for retry and nothing is applied. (A record whose state does not fit
/// fails alone; see the test below.)
#[test]
fn a_receipt_that_cannot_be_applied_is_refused_and_the_batch_stays_frozen() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    let frozen = c.freeze().unwrap().unwrap();
    let mut other_client = receipt(&mut c, 1, vec![authority(Some("B"), 2)]);
    other_client.client_id = "someone-else".into();
    let cases = vec![
        ("omitted record", receipt(&mut c, 1, vec![]), 1),
        (
            "extra only",
            receipt(&mut c, 1, vec![authority_of("x", Some("x"), 1)]),
            1,
        ),
        ("another client", other_client, 1),
        (
            "future batch",
            receipt(&mut c, 2, vec![authority(Some("B"), 2)]),
            2,
        ),
        (
            "sequence mismatch",
            receipt(&mut c, 1, vec![authority(Some("B"), 2)]),
            2,
        ),
        (
            "foreign rejection",
            rejecting(&mut c, 1, &[9], "denied", vec![authority(Some("B"), 2)]),
            1,
        ),
    ];
    for (name, r, sequence) in cases {
        assert!(c.acknowledge(sequence, r).is_err(), "{name} was accepted");
        assert_eq!(c.pending_count().unwrap(), 1, "{name}: the batch stays");
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B", "{name}");
        assert_eq!(
            c.record_stamp(&key()).unwrap(),
            0,
            "{name}: nothing applied"
        );
        assert_eq!(c.last_completed_push().unwrap(), 0, "{name}");
        let extra = schema().record_key("Entry", &json!({"id":"x"})).unwrap();
        assert!(c.read(&extra).unwrap().is_none(), "{name}: nothing applied");
    }
    assert_eq!(
        c.freeze().unwrap().unwrap(),
        frozen,
        "the frozen bytes are unchanged for retry"
    );
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 2)]);
    c.acknowledge(1, r).unwrap();
    assert_quiet(&mut c);
}

/// A duplicate receipt, before or after reopen, changes nothing and never
/// touches a later batch; completion is durable.
#[test]
fn duplicate_receipt_is_ignored_and_completion_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 2)]);
    c.acknowledge(1, r.clone()).unwrap();
    let again = c.acknowledge(1, r.clone()).unwrap();
    assert!(again.stale);
    c.transaction(|tx| tx.enqueue(mutation("C")).map(|_| ()))
        .unwrap();
    c.freeze().unwrap().unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(c.last_completed_push().unwrap(), 1);
    assert_eq!(c.pending_count().unwrap(), 1);
    let again = c.acknowledge(1, r).unwrap();
    assert!(again.stale, "an old receipt after reopen is a duplicate");
    assert_eq!(
        c.pending_count().unwrap(),
        1,
        "the later batch is untouched"
    );
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "C");
    let request = PushRequest::decode(&c.freeze().unwrap().unwrap()).unwrap();
    assert_eq!(request.batch_sequence, 2, "batch 2 is still in flight");
    let r = receipt(&mut c, 2, vec![authority(Some("C"), 3)]);
    c.acknowledge(2, r).unwrap();
    assert_quiet(&mut c);
}

/// The batch in flight is sent again after a restart, byte for byte, with the
/// declaration it was frozen with; completion then proceeds as usual.
#[test]
fn frozen_batch_and_its_declaration_survive_restart_until_completed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    seed(&mut c, "A");
    c.transaction(|tx| tx.enqueue(mutation("B")).map(|_| ()))
        .unwrap();
    let bytes = c.freeze().unwrap().unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(c.freeze().unwrap().unwrap(), bytes);
    let r = receipt(&mut c, 1, vec![authority(Some("B"), 2)]);
    c.acknowledge(1, r).unwrap();
    assert_quiet(&mut c);
    assert_eq!(
        c.read_sql("SELECT push_models FROM axton_client", &[])
            .unwrap()[0]["push_models"],
        serde_json::Value::Null,
        "the frozen declaration is released with the batch"
    );
}

/// Two mutations in one batch touching one record: the receipt carries the
/// final result once and both operations complete.
#[test]
fn repeated_record_in_one_batch_completes_from_one_final_result() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| {
        tx.enqueue(mutation("B"))?;
        tx.enqueue(mutation("C"))?;
        Ok(())
    })
    .unwrap();
    let batch = PushRequest::decode(&c.freeze().unwrap().unwrap()).unwrap();
    assert_eq!(batch.mutations.len(), 2);
    let r = receipt(&mut c, 1, vec![authority(Some("C!"), 3)]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "C!");
    assert_quiet(&mut c);
}

/// Divergence ([#122](https://github.com/zanminwang/axton/issues/122)): new
/// authority under which a queued operation no longer replays. The server's
/// row stays visible, the mutation stays queued and is still sent, the
/// application is told, and `record_status` marks the mutation until it
/// completes.
#[test]
fn a_pending_update_over_a_deleted_base_diverges_and_is_still_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    c.apply_page(page("book", 0, 1, Some("A"))).unwrap();
    let ordinal = c.transaction(|tx| tx.enqueue(mutation("B"))).unwrap();
    // Another record with its own pending edit is unaffected throughout.
    c.transaction(|tx| {
        tx.direct(create("Entry", "other", json!({"text":"o","note":null})))?;
        tx.enqueue(Mutation::new(
            "Edit",
            vec![Operation {
                model: "Entry".into(),
                op: OperationKind::Update,
                identity: json!({"id":"other"}),
                values: Some(json!({"text":"o2"})),
            }],
        ))
    })
    .unwrap();
    let other = schema()
        .record_key("Entry", &json!({"id":"other"}))
        .unwrap();
    // The server deleted the record: the update cannot replay over nothing.
    let report = c.apply_page(page("book", 1, 2, None)).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.diverged(), 1);
    let diverged = &report.reports[0];
    assert_eq!(diverged.kind, ReportKind::Diverged);
    assert_eq!(diverged.ordinal, Some(ordinal));
    assert_eq!((diverged.model.as_str(), diverged.stamp), ("Entry", 2));
    assert!(
        c.read(&key()).unwrap().is_none(),
        "the base (a deletion) is visible"
    );
    assert_eq!(
        c.pending_count().unwrap(),
        2,
        "the mutation is still queued"
    );
    let status = c.record_status(&key()).unwrap();
    assert_eq!(status["pending"][0]["ordinal"], ordinal);
    assert_eq!(status["pending"][0]["diverged"], true);
    assert_eq!(c.read(&other).unwrap().unwrap()["text"], "o2");
    assert_eq!(
        c.record_status(&other).unwrap()["pending"][0]["diverged"],
        false
    );
    // It is still sent, and its completion clears the mark.
    let batch = PushRequest::decode(&c.freeze().unwrap().unwrap()).unwrap();
    assert_eq!(batch.mutations.len(), 2);
    assert_eq!(
        c.record_status(&key()).unwrap()["pending"][0]["diverged"],
        true
    );
    let r = receipt(
        &mut c,
        1,
        vec![
            authority(Some("B"), 3),
            authority_of("other", Some("o2"), 1),
        ],
    );
    let completed = c.acknowledge(1, r).unwrap();
    assert!(completed.reports.is_empty());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert!(
        c.record_status(&key()).unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_quiet(&mut c);
}

/// A pending create over a record the server now has: the server's row is
/// visible, the create stays queued and diverged; the server decides.
#[test]
fn a_pending_create_over_an_existing_base_diverges() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    let ordinal = c.transaction(|tx| tx.enqueue(create_entry())).unwrap();
    assert_eq!(c.read(&created()).unwrap().unwrap()["text"], "new");
    let mut arrived = page("book", 0, 1, Some("theirs"));
    arrived.changes[0].identity = json!({"id":"n"});
    let report = c.apply_page(arrived).unwrap();
    assert_eq!(report.diverged(), 1);
    assert_eq!(report.reports[0].ordinal, Some(ordinal));
    assert_eq!(report.reports[0].identity, json!({"id":"n"}));
    assert_eq!(c.read(&created()).unwrap().unwrap()["text"], "theirs");
    assert_eq!(
        c.record_status(&created()).unwrap()["pending"][0]["diverged"],
        true
    );
    // A rejection removes the mutation and its mark; the server's row stays.
    c.freeze().unwrap().unwrap();
    let r = rejecting(&mut c, 1, &[ordinal], "entry.exists", vec![]);
    c.acknowledge(1, r).unwrap();
    assert_eq!(c.read(&created()).unwrap().unwrap()["text"], "theirs");
    assert!(
        c.record_status(&created()).unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(c.rejections().unwrap().len(), 1);
}

/// The same divergence through a receipt: the authority a receipt carries for
/// a record still edited afterwards is staged the same way and reported.
#[test]
fn divergence_is_reported_from_a_receipt_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    subscribe(&mut c, "book");
    c.apply_page(page("book", 0, 1, Some("A"))).unwrap();
    c.transaction(|tx| tx.enqueue(mutation("B"))).unwrap();
    c.freeze().unwrap().unwrap();
    let later = c.transaction(|tx| tx.enqueue(mutation("C"))).unwrap();
    // The server answered the first edit with a deletion; the later edit
    // cannot replay over it.
    let r = receipt(&mut c, 1, vec![authority(None, 5)]);
    let report = c.acknowledge(1, r).unwrap();
    assert_eq!(report.diverged(), 1);
    assert_eq!(report.reports[0].ordinal, Some(later));
    assert!(c.read(&key()).unwrap().is_none());
    assert_eq!(
        c.record_status(&key()).unwrap()["pending"][0]["diverged"],
        true
    );
}

/// A receipt record this client cannot apply fails alone, as on a page: it
/// is reported with its batch, the rest of the receipt lands and the batch
/// still completes, so the queue never stalls behind it.
#[test]
fn a_receipt_record_that_does_not_fit_is_skipped_and_the_batch_completes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    c.transaction(|tx| {
        tx.enqueue(mutation("hello"))?;
        tx.enqueue(create_entry())
    })
    .unwrap();
    c.freeze().unwrap().unwrap();
    let mut bad = authority(Some("Hello"), 3);
    bad.state = json!({"text":22,"note":null});
    let mut good = authority_of("n", Some("new"), 4);
    good.state = json!({"text":"new","note":null});
    let r = receipt(&mut c, 1, vec![bad, good]);
    let report = c.acknowledge(1, r).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.skipped(), 1);
    let skipped = &report.reports[0];
    assert_eq!(skipped.identity, json!({"id":"e"}));
    assert_eq!(skipped.detail["batch"], 1);
    assert_eq!(
        c.read(&key()).unwrap().unwrap()["text"],
        "A",
        "the record keeps the authority it had"
    );
    assert_eq!(c.record_stamp(&key()).unwrap(), 0, "and its stamp");
    assert_eq!(c.read(&created()).unwrap().unwrap()["text"], "new");
    assert_eq!(c.record_stamp(&created()).unwrap(), 4);
    assert_quiet(&mut c);
}
