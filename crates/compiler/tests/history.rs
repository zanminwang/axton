use axton_compiler::{check_fence, compile, reconcile_history};
#[test]
fn versions_retain_original_inputs() {
    let v1 =
        compile("model A { id UUID title String @@id(id) } mutation Save { a A.create }").unwrap();
    let history = reconcile_history(&v1, None).unwrap();
    let changed =
        compile("model A { id UUID title String count Int @@id(id) } mutation Save { a A.create }")
            .unwrap();
    assert!(reconcile_history(&changed, Some(&history)).is_err());
    let v2=compile("model A { id UUID title String count Int @@id(id) } mutation Save { a A.create @@version(2) }").unwrap();
    let next = reconcile_history(&v2, Some(&history)).unwrap();
    assert_eq!(
        next["mutations"]["Save"]["1"]["input"]["models"][0]["fields"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(reconcile_history(&v1, Some(&next)).is_err());
}
#[test]
fn nullable_addition_compatible_and_fence_blocks_removal() {
    let before =
        compile("model A { id UUID title String @@id(id) } mutation Save { a A.create }").unwrap();
    let history = reconcile_history(&before, None).unwrap();
    let after = compile(
        "model A { id UUID title String note String? @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    assert!(reconcile_history(&after, Some(&history)).is_ok());
    assert!(check_fence(&before["schema"], &after["schema"]).is_ok());
    assert!(check_fence(&after["schema"], &before["schema"]).is_err());
}

mod models {
    use axton_compiler::{check_fence, compile, reconcile_model_history};
    use serde_json::{Value, json};

    fn task(fields: &str, version: &str) -> Value {
        compile(&format!(
            "enum Status {{ open closed }} model Task {{ id UUID {fields} @@id(id) {version} }}"
        ))
        .unwrap()
    }
    fn snapshot<'a>(history: &'a Value, model: &str, version: u64) -> &'a Value {
        &history["models"][model][version.to_string()]
    }
    fn field_names(snapshot: &Value) -> Vec<&str> {
        snapshot["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn omitted_version_is_version_one_and_the_snapshot_keeps_the_contract() {
        let implicit = task("title String status Status", "");
        let explicit = task("title String status Status", "@@version(1)");
        assert_eq!(implicit["schema"]["models"][0]["version"], 1);
        assert_eq!(implicit["schema"], explicit["schema"]);
        let history = reconcile_model_history(&implicit, None).unwrap();
        assert_eq!(history["formatVersion"], 1);
        let v1 = snapshot(&history, "Task", 1);
        assert_eq!(v1["name"], "Task");
        assert_eq!(v1["version"], 1);
        assert_eq!(v1["identity"], json!(["id"]));
        assert_eq!(field_names(v1), ["id", "title", "status"]);
        assert_eq!(
            v1["fields"][2]["type"],
            json!({"kind":"enum","name":"Status"})
        );
        assert_eq!(
            v1["enums"],
            json!([{"name":"Status","values":["open","closed"]}])
        );
        assert_eq!(
            reconcile_model_history(&explicit, Some(&history)).unwrap(),
            history,
            "explicit v1 is the same contract"
        );
    }

    #[test]
    fn nullable_addition_keeps_the_version_and_updates_its_contract() {
        let history = reconcile_model_history(&task("title String", ""), None).unwrap();
        let next = reconcile_model_history(&task("title String note String?", ""), Some(&history))
            .unwrap();
        assert_eq!(
            field_names(snapshot(&next, "Task", 1)),
            ["id", "title", "note"]
        );
        assert_eq!(next["models"]["Task"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn breaking_output_changes_need_a_new_version_and_keep_the_old_contract() {
        let history =
            reconcile_model_history(&task("title String status Status", ""), None).unwrap();
        let breaking = [
            ("required field", "title String status Status count Int"),
            ("removed field", "status Status"),
            ("renamed field", "name String status Status"),
            ("changed type", "title Int status Status"),
        ];
        for (rule, fields) in breaking {
            let e = reconcile_model_history(&task(fields, ""), Some(&history)).unwrap_err();
            assert!(e.contains("Task v1"), "{rule}: {e}");
            assert!(e.contains("increase @@version"), "{rule}: {e}");
            let next = reconcile_model_history(&task(fields, "@@version(2)"), Some(&history))
                .unwrap_or_else(|e| panic!("{rule} with a bump: {e}"));
            assert_eq!(
                snapshot(&next, "Task", 1),
                snapshot(&history, "Task", 1),
                "{rule}: v1 stays as published"
            );
            assert_eq!(snapshot(&next, "Task", 2)["version"], 2);
        }
    }

    #[test]
    fn a_new_enum_value_needs_a_new_version_and_never_reaches_the_old_contract() {
        let v1 =
            compile("enum Status { open closed } model Task { id UUID status Status @@id(id) }")
                .unwrap();
        let history = reconcile_model_history(&v1, None).unwrap();
        let expanded = compile(
            "enum Status { open closed archived } model Task { id UUID status Status @@id(id) }",
        )
        .unwrap();
        let e = reconcile_model_history(&expanded, Some(&history)).unwrap_err();
        assert!(e.contains("Task v1") && e.contains("Status"), "{e}");
        let bumped = compile("enum Status { open closed archived } model Task { id UUID status Status @@id(id) @@version(2) }").unwrap();
        let next = reconcile_model_history(&bumped, Some(&history)).unwrap();
        assert_eq!(
            snapshot(&next, "Task", 1)["enums"],
            json!([{"name":"Status","values":["open","closed"]}])
        );
        assert_eq!(
            snapshot(&next, "Task", 2)["enums"],
            json!([{"name":"Status","values":["open","closed","archived"]}])
        );
        // A model that does not use the enum is untouched by its expansion.
        let other = compile("enum Status { open closed } model Task { id UUID status Status @@id(id) } model Note { id UUID text String @@id(id) }").unwrap();
        let history = reconcile_model_history(&other, None).unwrap();
        assert_eq!(snapshot(&history, "Note", 1)["enums"], json!([]));
        let other_expanded = compile("enum Status { open closed archived } model Task { id UUID status Status @@id(id) @@version(2) } model Note { id UUID text String @@id(id) }").unwrap();
        assert!(reconcile_model_history(&other_expanded, Some(&history)).is_ok());
    }

    #[test]
    fn identity_changes_are_refused_at_any_version() {
        let history = reconcile_model_history(&task("title String", ""), None).unwrap();
        let same = compile("model Task { id UUID title String @@id(id, title) }").unwrap();
        let bumped =
            compile("model Task { id UUID title String @@id(id, title) @@version(2) }").unwrap();
        for changed in [same, bumped] {
            let e = reconcile_model_history(&changed, Some(&history)).unwrap_err();
            assert!(e.contains("identity"), "{e}");
        }
    }

    #[test]
    fn history_misuse_follows_the_mutation_rules() {
        let history = reconcile_model_history(&task("title String", "@@version(2)"), None).unwrap();
        let decreased =
            reconcile_model_history(&task("title String", ""), Some(&history)).unwrap_err();
        assert!(decreased.contains("cannot decrease from 2"), "{decreased}");
        let removed = compile("model Other { id UUID @@id(id) }").unwrap();
        let e = reconcile_model_history(&removed, Some(&history)).unwrap_err();
        assert!(e.contains("retained model Task cannot be removed"), "{e}");
        for broken in [
            json!({"formatVersion":2,"models":{}}),
            json!({"formatVersion":1}),
            json!({"formatVersion":1,"models":{"Task":[]}}),
        ] {
            assert!(
                reconcile_model_history(&task("title String", "@@version(2)"), Some(&broken))
                    .is_err(),
                "{broken}"
            );
        }
        // A jump is allowed, like a mutation version.
        assert!(
            reconcile_model_history(&task("title String", "@@version(5)"), Some(&history)).is_ok()
        );
    }

    #[test]
    fn the_fence_defers_to_the_history_for_a_bumped_model() {
        let before = task("title String note String", "");
        let after = task("title String", "@@version(2)");
        assert!(
            check_fence(&before["schema"], &after["schema"]).is_ok(),
            "a bumped model may drop a field; its old contract is retained"
        );
        let unbumped = task("title String", "");
        assert!(check_fence(&before["schema"], &unbumped["schema"]).is_err());
        let gone = compile("model Other { id UUID @@id(id) }").unwrap();
        assert!(
            check_fence(&before["schema"], &gone["schema"]).is_err(),
            "a published model never disappears"
        );
    }
}
