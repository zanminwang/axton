use axton_core::*;
use serde_json::{Value, json};

fn schema() -> Schema {
    Schema::from_value(
        json!({"enums": [{"name":"Mood","values":["calm","busy"]}], "models":[{
            "name":"Entry", "identity":["id"], "fields":[
                {"name":"id","type":{"kind":"scalar","name":"uuid"},"nullable":false},
                {"name":"text","type":{"kind":"scalar","name":"string"},"nullable":false},
                {"name":"note","type":{"kind":"scalar","name":"string"},"nullable":true},
                {"name":"count","type":{"kind":"scalar","name":"int"},"nullable":false}
            ]
        }]}),
    )
    .unwrap()
}
const ID: &str = "01890F47-1234-7123-8123-123456789ABC";

#[test]
fn identities_are_exact_normalized_and_independent_of_channels() {
    let schema = schema();
    let key = schema.record_key("Entry", &json!({"id":ID})).unwrap();
    assert_eq!(key.identity, json!({"id":ID.to_lowercase()}));
    assert_eq!(
        key.encoded_identity().unwrap(),
        format!("{{\"id\":\"{}\"}}", ID.to_lowercase())
    );
    assert!(
        schema
            .record_key("Entry", &json!({"id":ID,"channel":"book"}))
            .is_err()
    );
    assert!(schema.record_key("Entry", &json!({"id":"bad"})).is_err());
}

#[test]
fn state_is_complete_but_patch_preserves_absent_and_null() {
    let s = schema();
    assert_eq!(
        s.normalize_state("Entry", &json!({"id":ID,"text":"a","count":0}))
            .unwrap(),
        json!({"text":"a","count":0,"note":null})
    );
    assert!(s.validate_state("Entry", &json!({"count":0})).is_err());
    assert_eq!(
        s.validate_patch("Entry", &json!({"note":null})).unwrap(),
        json!({"note":null})
    );
    assert_eq!(s.validate_patch("Entry", &json!({})).unwrap(), json!({}));
    assert!(s.validate_patch("Entry", &json!({"text":null})).is_err());
    assert!(s.validate_patch("Entry", &json!({"id":ID})).is_err());
    assert!(
        s.validate_patch("Entry", &json!({"count":9007199254740992u64}))
            .is_err()
    );
}

#[test]
fn independent_schemas_load_without_business_rust_types() {
    let second=Schema::from_value(json!({"enums":[],"models":[{"name":"Book","identity":["slug","edition"],"fields":[{"name":"slug","type":{"kind":"scalar","name":"string"},"nullable":false},{"name":"edition","type":{"kind":"scalar","name":"int"},"nullable":false}]}]})).unwrap();
    assert!(
        second
            .record_key("Book", &json!({"slug":"x","edition":1}))
            .is_ok()
    );
    assert!(
        schema()
            .record_key("Book", &json!({"slug":"x","edition":1}))
            .is_err()
    );
}

#[test]
fn a_page_names_its_channels_and_keeps_unknown_fields_out_of_the_records() {
    let page=PullPage::decode(br#"{"cursors":{"book:1":{"from":0,"to":2,"head":2}},"changes":[{"model":"Entry","identity":{"id":"x"},"stamp":2,"state":null}],"future":true}"#).unwrap();
    assert_eq!(page.channels().collect::<Vec<_>>(), ["book:1"]);
    assert_eq!(page.cursors["book:1"].to, 2);
    let wire: Value = serde_json::from_slice(&page.encode().unwrap()).unwrap();
    assert!(wire.get("scope").is_none());
    assert!(wire["changes"][0].get("syncId").is_none());
    assert!(
        wire["changes"][0].get("error").is_none(),
        "no error is no field"
    );
    assert!(
        PullPage::decode(br#"{"cursors":{"a":{"from":0,"to":9007199254740992,"head":9007199254740992}},"changes":[]}"#)
            .is_err()
    );
    assert!(
        PullPage::decode(br#"{"cursors":{"a":{"from":2,"to":1,"head":1}},"changes":[]}"#).is_err()
    );
}

#[test]
fn batch_envelope_keeps_unknown_data_in_canonical_bytes() {
    let a=PushRequest::decode(br#"{"clientId":"c","batchSequence":1,"models":{"Entry":1},"mutations":[{"ordinal":4,"name":"Edit","args":{}}],"future":1}"#).unwrap();
    let b=PushRequest::decode(br#"{"future":1,"mutations":[{"args":{},"name":"Edit","ordinal":4}],"batchSequence":1,"models":{"Entry":1},"clientId":"c"}"#).unwrap();
    assert_eq!(a.encode().unwrap(), b.encode().unwrap());
    assert_eq!(
        a.encode().unwrap(),
        br#"{"batchSequence":1,"clientId":"c","future":1,"models":{"Entry":1},"mutations":[{"args":{},"name":"Edit","ordinal":4}]}"#
    );
    assert_eq!(a.models.get("Entry"), Some(&1));
    let c=PushRequest::decode(br#"{"clientId":"c","batchSequence":1,"models":{"Entry":1},"mutations":[{"ordinal":4,"name":"Edit","args":{}}]}"#).unwrap();
    assert_ne!(a.encode().unwrap(), c.encode().unwrap());
    assert!(
        PushRequest::decode(
            br#"{"clientId":"c","batchSequence":1,"models":{"Entry":1},"mutations":[{"ordinal":1},{"ordinal":1}]}"#
        )
        .is_err()
    );
    assert!(
        PushRequest::decode(
            br#"{"clientId":"c","batchSequence":1,"mutations":[{"ordinal":1,"name":"Edit"}]}"#
        )
        .is_err(),
        "the read contracts the receipt is served at are required"
    );
}

#[test]
fn canonical_numbers_match_javascript_and_utf16_key_order() {
    assert_eq!(
        canonical_json(&json!({"z":1.0,"a":-0.0})).unwrap(),
        "{\"a\":0,\"z\":1}"
    );
    assert_eq!(
        canonical_json(&json!({"\u{e000}":1,"\u{1f600}":2})).unwrap(),
        "{\"😀\":2,\"\":1}"
    );
}

#[test]
fn receipt_wire_round_trips_and_carries_authority_without_a_cursor() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/receipt-authority.json"
    ))
    .unwrap();
    let canonical = &fixture["canonical"];
    let receipt = PushReceipt::decode(canonical["wire"].as_str().unwrap().as_bytes()).unwrap();
    assert_eq!(receipt.client_id, canonical["clientId"]);
    assert_eq!(
        receipt.batch_sequence,
        canonical["batchSequence"].as_u64().unwrap()
    );
    assert_eq!(
        receipt.records[0].stamp,
        canonical["stamp"].as_u64().unwrap()
    );
    assert!(receipt.rejections.is_empty());
    assert_eq!(
        PushReceipt::decode(&receipt.encode().unwrap()).unwrap(),
        receipt
    );
    assert_eq!(
        String::from_utf8(receipt.encode().unwrap()).unwrap(),
        canonical["wire"].as_str().unwrap(),
        "the canonical bytes are stable"
    );
    assert!(receipt.answers("device-1", 4));
    assert!(!receipt.answers("device-1", 5));
    assert!(!receipt.answers("device-2", 4));
    // A page change is the same type: a page's record decodes as a receipt's.
    let page = PullPage::decode(br#"{"cursors":{"a":{"from":8,"to":9,"head":9}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":12,"state":{"text":"Hello","note":null}}]}"#).unwrap();
    assert_eq!(page.changes[0], receipt.records[0]);
    let wire: Value = serde_json::from_slice(&receipt.encode().unwrap()).unwrap();
    assert!(wire["records"][0].get("syncId").is_none());
    assert!(wire.get("requiredCheckpoints").is_none());
    // A receipt never carries a read failure.
    assert!(PushReceipt::decode(br#"{"clientId":"d","batchSequence":1,"rejections":[],"records":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"error":"loader.failed"}]}"#).is_err());
}

#[test]
fn receipt_fixture_cases_decode_as_declared() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/receipt-authority.json"
    ))
    .unwrap();
    for case in fixture["receipt"].as_array().unwrap() {
        let wire = case["wire"].as_str().unwrap().as_bytes();
        let decoded = PushReceipt::decode(wire);
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {decoded:?}",
            case["name"]
        );
        if let Ok(receipt) = decoded {
            assert_eq!(
                PushReceipt::decode(&receipt.encode().unwrap()).unwrap(),
                receipt,
                "{}",
                case["name"]
            );
        }
    }
}

#[test]
fn received_state_supports_additive_schema_evolution() {
    let s = schema();
    assert_eq!(
        s.validate_state("Entry", &json!({"text":"a","count":0,"newField":42}))
            .unwrap(),
        json!({"text":"a","count":0,"note":null})
    );
    assert!(
        s.validate_state("Entry", &json!({"id":ID,"text":"a","count":0}))
            .is_err()
    );
}
#[test]
fn server_pull_request_accepts_js_integer_number_spellings() {
    for number in ["0.0", "1e0", "-0"] {
        let wire = format!("{{\"cursors\":{{\"s\":{number}}},\"models\":{{\"Entry\":1}}}}");
        assert!(PullRequest::decode(wire.as_bytes()).is_ok(), "{number}");
    }
}

#[test]
fn pull_page_fixture_cases_decode_as_declared() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/protocol/pull-page.json")).unwrap();
    let canonical = &fixture["canonical"];
    let page = PullPage::decode(canonical["wire"].as_str().unwrap().as_bytes()).unwrap();
    assert_eq!(
        page.channels().collect::<Vec<_>>(),
        canonical["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        page.changes.len(),
        canonical["changes"].as_u64().unwrap() as usize
    );
    assert_eq!(page.changes[2].error.as_deref(), Some("loader.failed"));
    assert!(!page.cursors["book:demo"].continues(), "at head");
    assert!(
        PullPage::decode(br#"{"cursors":{"a":{"from":0,"to":50,"head":80}},"changes":[]}"#)
            .unwrap()
            .cursors["a"]
            .continues()
    );
    assert!(page.changes[2].is_error() && page.changes[2].state.is_null());
    assert_eq!(
        String::from_utf8(page.encode().unwrap()).unwrap(),
        canonical["wire"].as_str().unwrap(),
        "the canonical bytes are stable"
    );
    for case in fixture["page"].as_array().unwrap() {
        let decoded = PullPage::decode(case["wire"].as_str().unwrap().as_bytes());
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {decoded:?}",
            case["name"]
        );
        if let Ok(page) = decoded {
            assert_eq!(
                PullPage::decode(&page.encode().unwrap()).unwrap(),
                page,
                "{}",
                case["name"]
            );
        }
    }
    for case in fixture["request"].as_array().unwrap() {
        let decoded = PullRequest::decode(case["wire"].as_str().unwrap().as_bytes());
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {decoded:?}",
            case["name"]
        );
    }
}

#[test]
fn shared_wire_fixtures_preserve_counter_boundaries() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/counter-boundaries.json"
    ))
    .unwrap();
    for case in fixture["pull"].as_array().unwrap() {
        let wire = case["wire"].as_str().unwrap().as_bytes();
        let valid = PullPage::decode(wire).is_ok();
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["name"]);
    }
}

#[test]
fn field_default_and_record_stamp_round_trip_and_axton_prefix_is_rejected() {
    let field: FieldDescriptor = serde_json::from_value(
        json!({"name":"rank","nullable":false,"type":{"kind":"scalar","name":"int"},"default":0}),
    )
    .unwrap();
    assert_eq!(field.default, Some(json!(0)));
    let plain: FieldDescriptor = serde_json::from_value(
        json!({"name":"t","nullable":true,"type":{"kind":"scalar","name":"string"}}),
    )
    .unwrap();
    assert_eq!(plain.default, None);
    assert!(!serde_json::to_string(&plain).unwrap().contains("default"));
    let page = PullPage::decode(
        br#"{"cursors":{"c":{"from":0,"to":1,"head":1}},"changes":[{"model":"E","identity":{"id":"e"},"stamp":7,"state":null}]}"#,
    )
    .unwrap();
    assert_eq!(page.changes[0].stamp, 7);
    assert!(
        String::from_utf8(page.encode().unwrap())
            .unwrap()
            .contains(r#""stamp":7"#)
    );
    let unstamped = PullPage::decode(
        br#"{"cursors":{"c":{"from":0,"to":1,"head":1}},"changes":[{"model":"E","identity":{"id":"e"},"state":null}]}"#,
    );
    assert!(unstamped.unwrap_err().to_string().contains("stamp"));
    let bad = Schema::from_value(
        json!({"enums":[],"models":[{"name":"axton_x","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}}]}]}),
    );
    assert!(bad.is_err());
    for name in ["sqlite_x", "SQLITE_x", "AXTON_x"] {
        let reserved = Schema::from_value(
            json!({"enums":[],"models":[{"name":name,"identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}}]}]}),
        );
        assert!(
            reserved.unwrap_err().to_string().contains("reserved"),
            "{name} must be refused as reserved"
        );
    }
    for name in ["sqlitex", "Sqlite", "axtonx"] {
        assert!(
            Schema::from_value(
                json!({"enums":[],"models":[{"name":name,"identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}}]}]}),
            )
            .is_ok(),
            "{name} must stay valid"
        );
    }
}

#[test]
fn scalar_and_enum_values_normalize_or_are_refused() {
    let s = Schema::from_value(json!({"enums":[{"name":"Mood","values":["calm","busy"]}],"models":[{
        "name":"E","identity":["id"],"fields":[
            {"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},
            {"name":"at","type":{"kind":"scalar","name":"dateTime"},"nullable":false},
            {"name":"ratio","type":{"kind":"scalar","name":"float"},"nullable":true},
            {"name":"mood","type":{"kind":"enum","name":"Mood"},"nullable":false},
            {"name":"tags","type":{"kind":"list","element":{"kind":"scalar","name":"string"}},"nullable":false}
        ]}]}))
    .unwrap();
    let patch = |v: Value| s.validate_patch("E", &v);
    // dateTime re-encodes to UTC milliseconds; a date, a space separator or a number is refused.
    assert_eq!(
        patch(json!({"at":"2024-01-02T03:04:05+01:00"})).unwrap(),
        json!({"at":"2024-01-02T02:04:05.000Z"})
    );
    assert_eq!(
        patch(json!({"at":"2024-01-02T03:04:05.25Z"})).unwrap()["at"],
        "2024-01-02T03:04:05.250Z"
    );
    for bad in [
        json!("2024-01-02"),
        json!("2024-01-02 03:04:05Z"),
        json!(1704164645),
    ] {
        assert!(patch(json!({"at":bad})).is_err(), "{bad} must be refused");
    }
    // float must be finite; -0 becomes 0; null is allowed only because ratio is nullable.
    assert_eq!(patch(json!({"ratio":-0.0})).unwrap()["ratio"], json!(0.0));
    assert_eq!(patch(json!({"ratio":1.5})).unwrap()["ratio"], json!(1.5));
    assert_eq!(patch(json!({"ratio":null})).unwrap()["ratio"], Value::Null);
    assert!(patch(json!({"ratio":"1.5"})).is_err());
    // JSON cannot carry NaN or infinity: `Value::from(f64::NAN)` is already null,
    // so the only non-finite inputs a wire can produce are refused as non-numbers.
    assert!(patch(json!({"ratio":"NaN"})).is_err());
    assert!(patch(json!({"ratio":"Infinity"})).is_err());
    // enum values must be declared and be strings.
    assert_eq!(patch(json!({"mood":"busy"})).unwrap()["mood"], "busy");
    assert!(patch(json!({"mood":"angry"})).is_err());
    assert!(patch(json!({"mood":1})).is_err());
    assert!(patch(json!({"mood":null})).is_err(), "mood is not nullable");
    // lists normalize each element and refuse non-lists and bad elements.
    assert_eq!(
        patch(json!({"tags":["a","b"]})).unwrap()["tags"],
        json!(["a", "b"])
    );
    assert!(patch(json!({"tags":"a"})).is_err());
    assert!(patch(json!({"tags":["a",1]})).is_err());
    assert!(patch(json!({"tags":null})).is_err(), "lists cannot be null");
}

#[test]
fn list_descriptors_must_hold_scalars_and_cannot_be_nullable() {
    let model = |field: Value| {
        Schema::from_value(
            json!({"enums":[{"name":"Mood","values":["calm"]}],"models":[{
            "name":"E","identity":["id"],"fields":[
                {"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},
                field
            ]}]}),
        )
    };
    assert!(model(json!({"name":"tags","type":{"kind":"list","element":{"kind":"scalar","name":"string"}},"nullable":false})).is_ok());
    let nullable_list = model(
        json!({"name":"tags","type":{"kind":"list","element":{"kind":"scalar","name":"string"}},"nullable":true}),
    );
    assert!(
        nullable_list
            .unwrap_err()
            .to_string()
            .contains("lists cannot be nullable")
    );
    let enum_list = model(
        json!({"name":"moods","type":{"kind":"list","element":{"kind":"enum","name":"Mood"}},"nullable":false}),
    );
    assert!(
        enum_list
            .unwrap_err()
            .to_string()
            .contains("list elements must be scalar")
    );
    let nested = model(
        json!({"name":"grid","type":{"kind":"list","element":{"kind":"list","element":{"kind":"scalar","name":"int"}}},"nullable":false}),
    );
    assert!(nested.is_err());
    assert!(
        model(json!({"name":"mood","type":{"kind":"enum","name":"Unknown"},"nullable":false}))
            .is_err()
    );
}

#[test]
fn push_batches_hold_one_to_twenty_mutations_with_distinct_ordinals() {
    let batch = |count: usize| {
        let mutations: Vec<Value> = (1..=count)
            .map(|i| json!({"ordinal":i,"name":"edit","operations":[]}))
            .collect();
        json!({"clientId":"c","batchSequence":1,"models":{"Entry":1},"mutations":mutations})
            .to_string()
    };
    assert!(PushRequest::decode(batch(0).as_bytes()).is_err());
    assert_eq!(
        PushRequest::decode(batch(1).as_bytes())
            .unwrap()
            .mutations
            .len(),
        1
    );
    assert_eq!(
        PushRequest::decode(batch(20).as_bytes())
            .unwrap()
            .mutations
            .len(),
        20
    );
    let err = PushRequest::decode(batch(21).as_bytes()).unwrap_err();
    assert!(err.to_string().contains("1..20"), "{err}");
    let duplicate = json!({"clientId":"c","batchSequence":1,"models":{"Entry":1},"mutations":[
        {"ordinal":1,"name":"edit","operations":[]},{"ordinal":1,"name":"edit","operations":[]}
    ]})
    .to_string();
    assert!(PushRequest::decode(duplicate.as_bytes()).is_err());
    let zero = json!({"clientId":"c","batchSequence":1,"mutations":[{"ordinal":0,"name":"edit","operations":[]}]}).to_string();
    assert!(PushRequest::decode(zero.as_bytes()).is_err());
}

#[test]
fn push_requests_refuse_a_blank_client_id_and_pulls_carry_none() {
    let mutations = json!([{"ordinal":1,"name":"edit","operations":[]}]);
    for blank in ["", "   "] {
        let push =
            json!({"clientId":blank,"batchSequence":1,"models":{"Entry":1},"mutations":mutations})
                .to_string();
        assert!(
            PushRequest::decode(push.as_bytes()).is_err(),
            "push {blank:?}"
        );
    }
    let push = json!({"clientId":"c","batchSequence":1,"models":{"Entry":1},"mutations":mutations})
        .to_string();
    assert_eq!(PushRequest::decode(push.as_bytes()).unwrap().client_id, "c");
    let missing = json!({"batchSequence":1,"mutations":mutations}).to_string();
    assert!(
        PushRequest::decode(missing.as_bytes()).is_err(),
        "missing clientId"
    );
    let pull = json!({"cursors":{"a":0},"models":{"Entry":1}}).to_string();
    let request = PullRequest::decode(pull.as_bytes()).unwrap();
    assert_eq!(
        String::from_utf8(request.encode().unwrap()).unwrap(),
        r#"{"cursors":{"a":0},"models":{"Entry":1}}"#,
        "a pull identifies no client"
    );
}

#[test]
fn shared_limits_are_defined_once_and_apply_per_channel() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/live-messages.json"
    ))
    .unwrap();
    assert_eq!(fixture["limits"]["pushMutations"], limits::PUSH_MUTATIONS);
    assert_eq!(fixture["limits"]["pushBytes"], limits::PUSH_BYTES);
    assert_eq!(fixture["limits"]["pullChanges"], limits::PULL_CHANGES);
    let page = |channels: usize, count: usize| {
        let changes: Vec<Value> = (1..=count)
            .map(
                |i| json!({"model":"Entry","identity":{"id":i.to_string()},"stamp":i,"state":null}),
            )
            .collect();
        let cursors: serde_json::Map<String, Value> = (0..channels)
            .map(|c| {
                (
                    format!("c{c}"),
                    json!({"from":0,"to":count.max(1),"head":count.max(1)}),
                )
            })
            .collect();
        json!({"cursors":cursors,"changes":changes}).to_string()
    };
    assert!(PullPage::decode(page(1, limits::PULL_CHANGES).as_bytes()).is_ok());
    let err = PullPage::decode(page(1, limits::PULL_CHANGES + 1).as_bytes()).unwrap_err();
    assert!(err.to_string().contains("exceeds 50"), "{err}");
    assert!(
        PullPage::decode(page(2, limits::PULL_CHANGES * 2).as_bytes()).is_ok(),
        "the cap is per channel"
    );
    assert!(PullPage::decode(page(2, limits::PULL_CHANGES * 2 + 1).as_bytes()).is_err());
}

#[test]
fn live_frames_decode_as_acknowledgement_or_page_and_channels_normalize() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/live-messages.json"
    ))
    .unwrap();
    for case in fixture["subscribe"].as_array().unwrap() {
        let wire = case["wire"].as_str().unwrap().as_bytes();
        match SubscribeRequest::decode(wire) {
            Ok(request) => {
                assert_eq!(case["valid"], true, "{}", case["name"]);
                assert_eq!(
                    json!(request.channels),
                    case["channels"],
                    "{}",
                    case["name"]
                );
                assert_eq!(json!(request.models), case["models"], "{}", case["name"]);
                let again = SubscribeRequest::decode(&request.encode().unwrap()).unwrap();
                assert_eq!(again, request, "encoding is canonical: {}", case["name"]);
            }
            Err(_) => assert_eq!(case["valid"], false, "{}", case["name"]),
        }
    }
    for case in fixture["acknowledgement"].as_array().unwrap() {
        let wire = case["wire"].as_str().unwrap().as_bytes();
        match SubscriptionAck::decode(wire) {
            Ok(ack) => {
                assert_eq!(case["valid"], true, "{}", case["name"]);
                assert_eq!(json!(ack.cursors), case["cursors"], "{}", case["name"]);
                assert_eq!(
                    SubscriptionAck::decode(&ack.encode().unwrap()).unwrap(),
                    ack
                );
            }
            Err(_) => assert_eq!(case["valid"], false, "{}", case["name"]),
        }
    }
    for case in fixture["frame"].as_array().unwrap() {
        let wire = case["wire"].as_str().unwrap().as_bytes();
        let kind = match LiveMessage::decode(wire) {
            Ok(LiveMessage::Acknowledged(_)) => "acknowledged",
            Ok(LiveMessage::Page(_)) => "page",
            Err(_) => "invalid",
        };
        assert_eq!(kind, case["kind"], "{}", case["name"]);
    }
    let models = std::collections::BTreeMap::from([("Task".to_string(), 1)]);
    let request = SubscribeRequest::new(vec!["b".into(), "a".into()], models.clone()).unwrap();
    let heads = |pairs: &[(&str, u64)]| {
        SubscriptionAck::new(pairs.iter().map(|(c, h)| (c.to_string(), *h)).collect()).unwrap()
    };
    assert!(heads(&[("a", 4), ("b", 0)]).confirms(&request));
    assert!(!heads(&[("a", 4)]).confirms(&request));
    assert!(!heads(&[("a", 4), ("b", 0), ("c", 1)]).confirms(&request));
    // The server's frame is the acknowledgement the client decodes, byte for byte.
    assert_eq!(
        String::from_utf8(heads(&[("b", 0), ("a", 4)]).encode().unwrap()).unwrap(),
        r#"{"cursors":{"a":4,"b":0},"type":"subscribed"}"#
    );
}

#[test]
fn pull_and_subscribe_declare_the_read_contracts_and_refuse_a_missing_or_bad_declaration() {
    // The declaration is the same object on both paths: one positive version per model.
    let good = json!({"cursors":{"a":0},"models":{"Task":2,"Note":1}});
    let request = PullRequest::decode(good.to_string().as_bytes()).unwrap();
    assert_eq!(request.models.get("Task"), Some(&2));
    assert_eq!(request.models.get("Note"), Some(&1));
    assert_eq!(
        String::from_utf8(request.encode().unwrap()).unwrap(),
        r#"{"cursors":{"a":0},"models":{"Note":1,"Task":2}}"#,
        "canonical: models sorted by name"
    );
    for (name, models) in [
        ("missing", Value::Null),
        ("not an object", json!(["Task"])),
        ("empty", json!({})),
        ("zero version", json!({"Task":0})),
        ("negative version", json!({"Task":-1})),
        ("fractional version", json!({"Task":1.5})),
        ("string version", json!({"Task":"1"})),
        ("empty model name", json!({"":1})),
    ] {
        let mut pull = good.clone();
        if models.is_null() {
            pull.as_object_mut().unwrap().remove("models");
        } else {
            pull["models"] = models.clone();
        }
        assert!(
            PullRequest::decode(pull.to_string().as_bytes()).is_err(),
            "pull {name}"
        );
        let mut subscribe = json!({"type":"subscribe","channels":["a"],"models":{"Task":1}});
        if models.is_null() {
            subscribe.as_object_mut().unwrap().remove("models");
        } else {
            subscribe["models"] = models;
        }
        assert!(
            SubscribeRequest::decode(subscribe.to_string().as_bytes()).is_err(),
            "subscribe {name}"
        );
    }
    let empty = std::collections::BTreeMap::new();
    assert!(SubscribeRequest::new(vec!["a".into()], empty).is_err());
}
