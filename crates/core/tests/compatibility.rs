//! Schema compatibility: what a local database built for one schema can
//! accept from a newer one without a rebuild.
use axton_core::{AdditiveStep, Compatibility, Schema};
use serde_json::{Value, json};

fn field(name: &str, ty: &str, nullable: bool) -> Value {
    json!({"name":name,"nullable":nullable,"type":{"kind":"scalar","name":ty}})
}
fn schema(models: Value, enums: Value) -> Schema {
    Schema::from_value(json!({"enums":enums,"models":models})).unwrap()
}
fn base() -> Schema {
    schema(
        json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",false)]}]),
        json!([]),
    )
}

#[test]
fn identical_and_additive_changes_are_classified() {
    let b = base();
    assert_eq!(Schema::compatibility(&b, &b), Compatibility::Identical);
    let nullable = schema(
        json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",false),field("note","string",true)]}]),
        json!([]),
    );
    assert_eq!(
        Schema::compatibility(&b, &nullable),
        Compatibility::Additive(vec![AdditiveStep::AddField {
            model: "Task".into(),
            field: "note".into()
        }])
    );
    let defaulted = schema(
        json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",false),{"name":"rank","nullable":false,"type":{"kind":"scalar","name":"int"},"default":0}]}]),
        json!([]),
    );
    assert!(matches!(
        Schema::compatibility(&b, &defaulted),
        Compatibility::Additive(_)
    ));
    let with_model = schema(
        json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",false)]},
               {"name":"Note","identity":["id"],"fields":[field("id","string",false)]}]),
        json!([]),
    );
    assert_eq!(
        Schema::compatibility(&b, &with_model),
        Compatibility::Additive(vec![AdditiveStep::AddModel("Note".into())])
    );
    // Reordering fields is not a change.
    let reordered = schema(
        json!([{"name":"Task","identity":["id"],"fields":[field("title","string",false),field("id","string",false)]}]),
        json!([]),
    );
    assert_eq!(
        Schema::compatibility(&b, &reordered),
        Compatibility::Identical
    );
}

#[test]
fn every_incompatible_kind_names_its_reason() {
    let b = base();
    let cases: Vec<(&str, Schema, &str)> = vec![
        (
            "version bump",
            schema(
                json!([{"name":"Task","version":2,"identity":["id"],"fields":[field("id","string",false),field("title","string",false)]}]),
                json!([]),
            ),
            "changed version",
        ),
        (
            "required field",
            schema(
                json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",false),field("due","string",false)]}]),
                json!([]),
            ),
            "required and has no default",
        ),
        (
            "removed field",
            schema(
                json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false)]}]),
                json!([]),
            ),
            "removed or renamed",
        ),
        (
            "retyped field",
            schema(
                json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","int",false)]}]),
                json!([]),
            ),
            "changed its type",
        ),
        (
            "nullability",
            schema(
                json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",true)]}]),
                json!([]),
            ),
            "changed its type or nullability",
        ),
        (
            "identity",
            schema(
                json!([{"name":"Task","identity":["id","title"],"fields":[field("id","string",false),field("title","string",false)]}]),
                json!([]),
            ),
            "changed its identity",
        ),
        (
            "unique",
            schema(
                json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),field("title","string",false)],"unique":[["title"]]}]),
                json!([]),
            ),
            "unique constraints",
        ),
        (
            "removed model",
            schema(
                json!([{"name":"Other","identity":["id"],"fields":[field("id","string",false)]}]),
                json!([]),
            ),
            "was removed",
        ),
    ];
    for (name, incoming, expected) in cases {
        match Schema::compatibility(&b, &incoming) {
            Compatibility::Incompatible(reason) => {
                assert!(reason.contains(expected), "{name}: {reason}")
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn enum_values_used_by_a_stored_field_must_not_change_but_new_enums_may_appear() {
    let with_enum = |values: Value, extra_enum: bool| {
        let mut enums = vec![json!({"name":"Mood","values":values})];
        if extra_enum {
            enums.push(json!({"name":"Kind","values":["a"]}));
        }
        schema(
            json!([{"name":"Task","identity":["id"],"fields":[field("id","string",false),{"name":"mood","nullable":false,"type":{"kind":"enum","name":"Mood"}}]}]),
            Value::Array(enums),
        )
    };
    let stored = with_enum(json!(["calm", "busy"]), false);
    assert_eq!(
        Schema::compatibility(&stored, &with_enum(json!(["calm", "busy"]), true)),
        Compatibility::Identical
    );
    match Schema::compatibility(&stored, &with_enum(json!(["calm", "busy", "tired"]), false)) {
        Compatibility::Incompatible(reason) => assert!(reason.contains("enum Mood"), "{reason}"),
        other => panic!("{other:?}"),
    }
}
