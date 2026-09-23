//! The host operation contract: the shared fixture round-trips through the
//! Rust types, and a malformed request or response is refused per operation.
use axton_server::host::{
    Acknowledged, Claimed, Handled, Head, HostRequest, Invalidation, Loaded, OPERATIONS,
    PublicationIntent, Published, RecordRef, Scanned, Stamped,
};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/protocol/host-operations.json"
    ))
    .expect("fixture is valid JSON")
}

/// Decode one response into the type its operation promises and re-encode it.
fn round_trip_response(op: &str, value: &Value) -> Result<Value, String> {
    macro_rules! round {
        ($type:ty) => {
            serde_json::from_value::<$type>(value.clone())
                .map(|decoded| serde_json::to_value(decoded).expect("re-encodes"))
                .map_err(|error| error.to_string())
        };
    }
    match op {
        "claim" => round!(Claimed),
        "saveReceipt" | "savepoint" | "rollback" | "release" => round!(Acknowledged),
        "head" => round!(Head),
        "scan" => round!(Scanned),
        "handle" => round!(Handled),
        "load" => round!(Loaded),
        "advanceStamp" | "ensureStamp" => round!(Stamped),
        "publish" => round!(Published),
        other => panic!("no response type is wired for {other}"),
    }
}

/// The variants serde itself knows about, read out of its own refusal. Keeps
/// [`OPERATIONS`] honest when a variant is added.
fn variants_serde_accepts() -> Vec<String> {
    let error = serde_json::from_value::<HostRequest>(json!({"op": "\u{0}unknown"}))
        .expect_err("an unknown op is refused")
        .to_string();
    let (_, listed) = error
        .split_once("expected one of ")
        .expect("lists variants");
    listed
        .split(", ")
        .map(|name| name.trim_matches(|c| c == '`' || c == ' ').to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

#[test]
fn the_operation_list_matches_the_request_enum() {
    assert_eq!(variants_serde_accepts(), OPERATIONS);
}

#[test]
fn the_fixture_covers_every_operation_exactly_once() {
    let fixture = fixture();
    let covered: Vec<String> = fixture["operations"]
        .as_array()
        .expect("operations is an array")
        .iter()
        .map(|entry| entry["op"].as_str().expect("op is a string").to_string())
        .collect();
    let mut sorted = covered.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), covered.len(), "an operation appears twice");
    let mut expected: Vec<String> = OPERATIONS.iter().map(|op| op.to_string()).collect();
    expected.sort();
    assert_eq!(sorted, expected);
}

#[test]
fn every_fixture_request_and_response_round_trips() {
    let fixture = fixture();
    for entry in fixture["operations"].as_array().unwrap() {
        let op = entry["op"].as_str().unwrap();
        let request = &entry["request"];
        let decoded: HostRequest = serde_json::from_value(request.clone())
            .unwrap_or_else(|error| panic!("{op} request: {error}"));
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            *request,
            "{op} request does not re-encode to the fixture"
        );
        let responses = entry["responses"].as_array().unwrap();
        assert!(!responses.is_empty(), "{op} has no response example");
        for response in responses {
            let variant = response["variant"].as_str().unwrap();
            let value = &response["value"];
            let encoded = round_trip_response(op, value)
                .unwrap_or_else(|error| panic!("{op}/{variant} response: {error}"));
            assert_eq!(
                encoded, *value,
                "{op}/{variant} response does not re-encode to the fixture"
            );
        }
    }
}

#[test]
fn a_request_missing_a_field_or_carrying_an_unknown_one_is_refused() {
    let fixture = fixture();
    for entry in fixture["operations"].as_array().unwrap() {
        let op = entry["op"].as_str().unwrap();
        let request = entry["request"].as_object().unwrap();
        let mut unknown = request.clone();
        unknown.insert("surprise".into(), json!(1));
        assert!(
            serde_json::from_value::<HostRequest>(Value::Object(unknown)).is_err(),
            "{op} accepted an unknown request field"
        );
        for field in request.keys().filter(|key| *key != "op") {
            let mut missing = request.clone();
            missing.remove(field);
            assert!(
                serde_json::from_value::<HostRequest>(Value::Object(missing)).is_err(),
                "{op} accepted a request without {field}"
            );
            // `arguments`, `identity` and `identities` carry schema-shaped
            // payloads verbatim; the contract constrains every other field.
            if ["arguments", "identity", "identities"].contains(&field.as_str()) {
                continue;
            }
            let mut wrong = request.clone();
            wrong.insert(field.clone(), json!(true));
            assert!(
                serde_json::from_value::<HostRequest>(Value::Object(wrong)).is_err(),
                "{op} accepted a boolean {field}"
            );
        }
    }
    assert!(serde_json::from_value::<HostRequest>(json!({"op": "vacuum"})).is_err());
}

#[test]
fn a_response_carrying_an_unknown_field_is_refused() {
    let fixture = fixture();
    for entry in fixture["operations"].as_array().unwrap() {
        let op = entry["op"].as_str().unwrap();
        for response in entry["responses"].as_array().unwrap() {
            let value = &response["value"];
            let surprised = match value {
                Value::Object(fields) => {
                    let mut fields = fields.clone();
                    fields.insert("surprise".into(), json!(1));
                    Value::Object(fields)
                }
                Value::Array(rows) if !rows.is_empty() => {
                    // `load` entries are opaque record state; `scan` rows are not.
                    if op != "scan" {
                        continue;
                    }
                    let mut rows = rows.clone();
                    let mut first = rows[0].as_object().unwrap().clone();
                    first.insert("surprise".into(), json!(1));
                    rows[0] = Value::Object(first);
                    Value::Array(rows)
                }
                _ => continue,
            };
            assert!(
                round_trip_response(op, &surprised).is_err(),
                "{op} accepted an unknown response field"
            );
        }
    }
}

#[test]
fn a_response_of_the_wrong_type_is_refused_per_operation() {
    // One clearly wrong answer per operation, in the shape a host might drift into.
    let wrong: [(&str, Value); 12] = [
        ("claim", json!({"clientId":"c","owner":"o","sequence":-1})),
        ("saveReceipt", json!({"saved": true})),
        ("head", json!("7")),
        ("scan", json!({"rows": []})),
        ("savepoint", json!({})),
        ("rollback", json!({})),
        ("release", json!({})),
        ("handle", json!({"channel": "shared"})),
        ("load", json!({"0": null})),
        ("advanceStamp", json!("4")),
        ("ensureStamp", json!(0)),
        ("publish", json!({"cursor": 0, "stamp": 1})),
    ];
    for (op, value) in wrong {
        assert!(
            round_trip_response(op, &value).is_err(),
            "{op} accepted {value}"
        );
    }
}

#[test]
fn a_handle_response_carries_changes_and_publications_or_a_rejection_and_never_both() {
    let task = |id: &str| RecordRef {
        model: "Task".into(),
        identity: json!({"id": id}),
    };
    assert_eq!(
        serde_json::from_value::<Handled>(json!({
            "changes": [{"model":"Task","identity":{"id":"t-2"}}],
            "publications": [{"channel":"shared"}, {"channel":"other","records":[]}]
        }))
        .unwrap(),
        Handled::Settled {
            changes: vec![task("t-2")],
            publications: vec![
                PublicationIntent {
                    channel: "shared".into(),
                    records: None
                },
                PublicationIntent {
                    channel: "other".into(),
                    records: Some(vec![])
                }
            ]
        }
    );
    assert_eq!(
        serde_json::from_value::<Handled>(json!({"changes": [], "publications": []})).unwrap(),
        Handled::Settled {
            changes: vec![],
            publications: vec![]
        },
        "a handler that changed nothing beyond its operations and published nothing"
    );
    assert_eq!(
        serde_json::from_value::<Handled>(json!({"rejection": "task.refused"})).unwrap(),
        Handled::Rejected {
            rejection: "task.refused".into()
        }
    );
    let both = serde_json::from_value::<Handled>(
        json!({"changes": [], "publications": [], "rejection": "task.refused"}),
    )
    .unwrap_err()
    .to_string();
    assert!(both.contains("not several"), "{both}");
    assert_eq!(
        serde_json::from_value::<Handled>(json!({"error": "boom"})).unwrap(),
        Handled::Failed {
            error: "boom".into()
        }
    );
    for refused in [
        json!({"error": 1}),
        json!({"error": "boom", "rejection": "x"}),
        json!({"error": "boom", "changes": [], "publications": []}),
    ] {
        assert!(
            serde_json::from_value::<Handled>(refused.clone()).is_err(),
            "accepted {refused}"
        );
    }
    let none = serde_json::from_value::<Handled>(json!({}))
        .unwrap_err()
        .to_string();
    assert!(none.contains("invalid handler settlement"), "{none}");
    for refused in [
        json!({}),
        json!({"changes": []}),
        json!({"publications": []}),
        json!({"channel": "shared"}),
        json!({"changes": null, "publications": []}),
        json!({"changes": [{"model":"","identity":{}}], "publications": []}),
        json!({"changes": [{"model":"Task","identity":"t"}], "publications": []}),
        json!({"changes": [], "publications": [{"channel":""}]}),
        json!({"changes": [], "publications": [{"channel":"shared","records":[{"model":"Task"}]}]}),
        json!({"changes": [], "publications": [{"scope":"shared"}]}),
        json!({"rejection": null}),
        json!({"rejection": "Not A Code"}),
        json!({"settled": "shared"}),
    ] {
        assert!(
            serde_json::from_value::<Handled>(refused.clone()).is_err(),
            "accepted {refused}"
        );
    }
}

#[test]
fn a_load_response_is_rows_or_a_refusal_code() {
    assert_eq!(
        serde_json::from_value::<Loaded>(json!([{"id":"t-1"}, null])).unwrap(),
        Loaded::Rows(vec![Some(json!({"id":"t-1"})), None])
    );
    assert_eq!(
        serde_json::from_value::<Loaded>(json!({"rejection":"task.forbidden"})).unwrap(),
        Loaded::Refused {
            rejection: "task.forbidden".into()
        }
    );
    assert_eq!(
        serde_json::from_value::<Loaded>(json!({"error": "boom"})).unwrap(),
        Loaded::Failed {
            error: "boom".into()
        }
    );
    for refused in [
        json!({}),
        json!({"rejection": ""}),
        json!({"rejection": "Not A Code"}),
        json!({"rows": []}),
        json!(null),
        json!({"error": 1}),
        json!({"error": "boom", "rejection": "x"}),
    ] {
        assert!(
            serde_json::from_value::<Loaded>(refused.clone()).is_err(),
            "accepted {refused}"
        );
    }
}

#[test]
fn counters_keep_their_range_and_name_themselves() {
    let row = |stamp: Value| {
        let mut row = json!({"channel":"a","cursor":1,"model":"Entry","identity":{"id":"e"},"identityKey":"{\"id\":\"e\"}"});
        row["stamp"] = stamp;
        serde_json::from_value::<Invalidation>(row).map_err(|error| error.to_string())
    };
    assert_eq!(row(json!(7)).unwrap().stamp, 7);
    // `read_counter`'s tolerance for integral JSON numbers is preserved.
    assert_eq!(row(json!(7.0)).unwrap().stamp, 7);
    for bad in [json!(0), json!(-1), json!(1.5), json!(9007199254740992u64)] {
        let error = row(bad.clone()).unwrap_err();
        assert!(error.contains("stamp"), "{bad}: {error}");
    }
    assert_eq!(
        serde_json::from_value::<Head>(json!(0)).unwrap(),
        Head(0),
        "a head of zero is a real answer"
    );
    assert!(serde_json::from_value::<Head>(json!(-1)).is_err());
    assert_eq!(
        serde_json::from_value::<Claimed>(
            json!({"clientId":"c","owner":"o","sequence":0,"receipt":null})
        )
        .unwrap()
        .receipt,
        None
    );
    assert_eq!(
        serde_json::from_value::<Claimed>(json!({"clientId":"c","owner":"o","sequence":0}))
            .unwrap()
            .receipt,
        None,
        "an absent receipt has always meant the same as a null one"
    );
}

#[test]
fn an_unusable_response_names_its_operation_and_ordinal() {
    let handle = HostRequest::Handle {
        name: "edit".into(),
        version: 1,
        arguments: json!({}),
        owner: "alice".into(),
        ordinal: 3,
    };
    let error = handle.invalid_response("invalid handler settlement");
    assert_eq!(error.code, axton_server::code::HANDLER_INVALID);
    assert_eq!(
        error.message,
        "handle(ordinal 3) response invalid: invalid handler settlement"
    );
    let load = HostRequest::Load {
        model: "Task".into(),
        version: 1,
        identities: vec![],
        owner: "alice".into(),
    };
    assert_eq!(
        load.invalid_response("x").code,
        axton_server::code::LOADER_INVALID
    );
    assert_eq!(
        HostRequest::Claim {
            owner: "alice".into(),
            client_id: "c".into()
        }
        .invalid_response("x")
        .code,
        axton_server::code::STORAGE_INVALID
    );
    assert_eq!(
        HostRequest::Head {
            channel: "shared".into()
        }
        .invalid_response("x")
        .code,
        axton_server::code::HOST_INVALID
    );
    assert_eq!(
        HostRequest::AdvanceStamp {
            model: "Task".into(),
            identity_key: "{\"id\":\"t-1\"}".into()
        }
        .invalid_response("x")
        .code,
        axton_server::code::HOST_INVALID
    );
}
