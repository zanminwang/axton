use axton_sim::scenario05::run_recovery05;
#[test]
fn receipt_and_authority_permutations_reopen_every_persisted_boundary() {
    for (authority_first, private) in [(false, false), (true, false), (false, true)] {
        let directory = tempfile::tempdir().unwrap();
        let trace = run_recovery05(&directory.path().join("db"), authority_first, private);
        assert!(trace.exact_retry, "frozen bytes changed after reopen");
        assert!(
            trace.unfinished_after_receipt,
            "content or receipt alone fabricated coverage"
        );
        assert!(trace.durable_completion, "completion lost original waiter");
        assert_eq!(
            trace.optimistic["text"], "later",
            "a later pending Mutation stays on top of the settled record"
        );
        assert_eq!(
            trace.visible["text"], "canonical",
            "the private receipt or the Stream authority settled beneath it"
        );
        assert_eq!(trace.boundaries.len(), 5);
    }
}
