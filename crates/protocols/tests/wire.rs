use axton_protocols::wire::*;
use serde_json::{Value, json};

fn ctx() -> RequestContext {
    RequestContext {
        protocol: PROTOCOL,
        store_id: "store".into(),
        stream: "User:a".into(),
        materialization: "schema1".into(),
    }
}
fn request() -> MutationRequest {
    let mut request: MutationRequest = serde_json::from_value(wire_form(json!({
        "context":ctx(), "batchId":1,"digest":"", "mutations":[{
            "id":1,"name":"Write","version":1,"descriptor":"retained1","operations":[
                {"step":0,"inputPath":"text","operation":"argument","model":null,"identity":null,"value":"hello"},
                {"step":1,"inputPath":"items","operation":"argument","model":null,"identity":null,"value":[]},
                {"step":2,"inputPath":"optional","operation":"argument","model":null,"identity":null,"value":null}
            ]}]}))).unwrap();
    request.digest = batch_digest(&request).unwrap();
    request
}
fn ack(request: &MutationRequest) -> BatchAcknowledgement {
    serde_json::from_value(wire_form(json!({"context":request.context,"batchId":1,"digest":request.digest,"results":[
        {"mutationId":1,"outcome":{"kind":"accepted","syncCursor":0,"materialization":"schema1","result":{"appOpaque":[1,null]},"targets":[]}}
    ]}))).unwrap()
}
fn wire_form(mut value: Value) -> Value {
    if let Some(context) = value.as_object_mut().unwrap().remove("context") {
        value
            .as_object_mut()
            .unwrap()
            .extend(context.as_object().unwrap().clone());
    }
    value
}
fn rejects<T: serde::de::DeserializeOwned + Validate>(value: Value) {
    assert!(
        decode::<T>(&serde_json::to_vec(&value).unwrap()).is_err(),
        "accepted {value}"
    );
}

#[test]
fn a1_canonical_batch_binds_exact_order_context_and_descriptor() {
    let request = request();
    validate_batch(&request).unwrap();
    let bytes = encode(&request).unwrap();
    assert_eq!(decode::<MutationRequest>(&bytes).unwrap(), request);
    let mut changed = request.clone();
    changed.mutations[0].operations[0].value = json!("changed");
    assert!(validate_batch(&changed).is_err());
    changed.digest = batch_digest(&changed).unwrap();
    assert_ne!(changed.digest, request.digest);
    for modify in [0, 1, 2] {
        let mut changed = request.clone();
        match modify {
            0 => changed.context.stream = "User:b".into(),
            1 => changed.context.store_id = "new".into(),
            _ => changed.mutations[0].descriptor = "retained2".into(),
        }
        assert_ne!(batch_digest(&changed).unwrap(), request.digest);
        assert!(validate_batch(&changed).is_err());
    }
}
#[test]
fn batch_digest_survives_a_protocol_number_change() {
    // The protocol number is transport: a Batch frozen under one protocol
    // keeps its digest under the next, while admission still checks it.
    let request = request();
    for protocol in [PROTOCOL - 1, PROTOCOL + 1] {
        let mut moved = request.clone();
        moved.context.protocol = protocol;
        assert_eq!(batch_digest(&moved).unwrap(), request.digest);
        assert!(validate_batch(&moved).is_err());
    }
}
#[test]
fn strict_envelopes_counters_and_duplicates() {
    let request = request();
    let value = serde_json::to_value(&request).unwrap();
    for path in ["top", "context", "mutation", "operation"] {
        let mut bad = value.clone();
        match path {
            "top" => bad["extra"] = json!(1),
            "context" => bad["contextExtra"] = json!(1),
            "mutation" => bad["mutations"][0]["extra"] = json!(1),
            _ => bad["mutations"][0]["operations"][0]["extra"] = json!(1),
        }
        rejects::<MutationRequest>(bad);
    }
    let mut bad = value.clone();
    bad["protocol"] = json!(5);
    rejects::<MutationRequest>(bad);
    let mut bad = value.clone();
    bad["batchId"] = json!(9_007_199_254_740_992u64);
    rejects::<MutationRequest>(bad);
    let mut duplicate = request.clone();
    duplicate.mutations.push(duplicate.mutations[0].clone());
    assert!(batch_digest(&duplicate).is_err());
    let mut duplicate = request.clone();
    let op = duplicate.mutations[0].operations[0].clone();
    duplicate.mutations[0].operations.push(op);
    assert!(batch_digest(&duplicate).is_err());
    let mut bad = value;
    bad["mutations"][0]["operations"][0]["model"] = json!("Entry");
    rejects::<MutationRequest>(bad);
}
#[test]
fn exact_ack_and_owned_target_envelopes() {
    let request = request();
    let acknowledgement = ack(&request);
    validate_acknowledgement(&request, &acknowledgement, |_, _| true).unwrap();
    for variant in 0..6 {
        let mut bad = acknowledgement.clone();
        match variant {
            0 => bad.results.clear(),
            1 => bad.results.push(bad.results[0].clone()),
            2 => bad.results[0].mutation_id = 2,
            3 => bad.context.stream = "User:b".into(),
            4 => bad.context.store_id = "new".into(),
            _ => bad.digest = "0".repeat(64),
        }
        assert!(validate_acknowledgement(&request, &bad, |_, _| true).is_err());
    }
    // A receipt is correlated by Store, Stream, Batch and digest: one that
    // answers in another materialization still acknowledges the Batch.
    let mut moved = acknowledgement.clone();
    moved.context.materialization = "schema2".into();
    validate_acknowledgement(&request, &moved, |_, _| true).unwrap();
    let base = json!({"kind":"stream","key":{"model":"Entry","identity":{"id":"e"}},"cursor":2,"fallback":{"key":{"model":"Entry","identity":{"id":"e"}},"cursor":null,"state":{"text":"a"}}});
    let target = decode::<SettlementTarget>(&serde_json::to_vec(&base).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(target).unwrap(), base);
    for variant in 0..5 {
        let mut bad = base.clone();
        match variant {
            0 => bad["key"]["extra"] = json!(true),
            1 => bad["fallback"]["cursor"] = json!(1),
            2 => bad["fallback"]
                .as_object_mut()
                .unwrap()
                .remove("cursor")
                .map(|_| ())
                .unwrap(),
            3 => bad["fallback"]["key"]["identity"] = json!({"id":"other"}),
            _ => bad["cursor"] = json!(0),
        };
        rejects::<SettlementTarget>(bad);
    }
    rejects::<BatchAcknowledgement>(wire_form(
        json!({"context":ctx(),"batchId":1,"digest":request.digest,"results":[{"mutationId":1,"outcome":{"kind":"accepted","syncCursor":0,"materialization":"schema1","result":null,"targets":[],"extra":true}}]}),
    ));
    rejects::<BatchAcknowledgement>(wire_form(
        json!({"context":ctx(),"batchId":1,"digest":request.digest,"results":[{"mutationId":1,"outcome":{"kind":"accepted","syncCursor":0,"result":null,"targets":[]}}]}),
    ));
    rejects::<BatchAcknowledgement>(wire_form(
        json!({"context":ctx(),"batchId":1,"digest":request.digest,"results":[{"mutationId":1,"outcome":{"kind":"accepted","syncCursor":0,"materialization":"","result":null,"targets":[]}}]}),
    ));
}
#[test]
fn a14_operations_preserve_null_empty_omitted_order_and_retained_descriptor() {
    let request = request();
    let input = reconstruct_input(&request.mutations[0].operations).unwrap();
    assert_eq!(input, json!({"text":"hello","items":[],"optional":null}));
    assert!(input.get("omitted").is_none());
    let operations:Vec<MutationOperation>=serde_json::from_value(json!([
        {"step":0,"inputPath":"items[0]","operation":"create","model":"Entry","identity":{"id":"a"},"value":{"text":"A"}},
        {"step":1,"inputPath":"items[1]","operation":"delete","model":"Entry","identity":{"id":"b"},"value":null}
    ])).unwrap();
    assert_eq!(
        reconstruct_input(&operations).unwrap(),
        json!({"items":[{"id":"a","text":"A"},{"id":"b"}]})
    );
    let mut gap = operations.clone();
    gap[1].input_path = "items[2]".into();
    assert!(reconstruct_input(&gap).is_err());
    let mut overlap = operations;
    overlap[1].input_path = "items[0]".into();
    assert!(reconstruct_input(&overlap).is_err());
}
#[test]
fn strict_read_and_delta_context() {
    let read = wire_form(
        json!({"context":ctx(),"requestId":"read1","outcome":{"kind":"succeeded","result":{"arbitrary":true}},"records":[{"key":{"model":"Entry","identity":{"id":"e"}},"cursor":null,"state":null}]}),
    );
    let response = decode::<ReadResponse>(&serde_json::to_vec(&read).unwrap()).unwrap();
    response.context.admit(&ctx()).unwrap();
    let mut bad = read.clone();
    bad["records"][0]["state"] = json!("bad");
    rejects::<ReadResponse>(bad);
    let mut bad = read;
    bad["records"][0]["key"]["unknown"] = json!(true);
    rejects::<ReadResponse>(bad);
    let value = wire_form(json!({"context":ctx(),"after":0,"through":40,"bootstrap":true}));
    let delta = decode::<DeltaRequest>(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(delta.after, 0);
    let mut bad = value;
    bad["through"] = json!(9_007_199_254_740_992u64);
    rejects::<DeltaRequest>(bad);
}
fn frozen() -> FrozenDelivery {
    let changes = vec![
        AuthorityChange::Record {
            key: RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"a"}),
            },
            cursor: 4,
            state: json!({"text":"a"}),
        },
        AuthorityChange::Record {
            key: RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"b"}),
            },
            cursor: 4,
            state: json!({"text":"b"}),
        },
    ];
    let units = plan_units(&changes, &Default::default(), &[], 0, 4, 4).unwrap();
    freeze_delivery(
        ctx(),
        "plan".into(),
        DeliveryPurpose::Sync,
        0,
        4,
        4,
        100,
        units,
        1,
    )
    .unwrap()
}
#[test]
fn delivery_manifest_binds_coverage_payload_parts_and_complete_units() {
    let frozen = frozen();
    let mut p = DeliveryProgress::new(&frozen.header, &ctx(), 0).unwrap();
    assert_eq!(frozen.parts.len(), 2);
    let before = p.clone();
    assert!(p.stage_with_limit(&frozen.parts[0], &ctx(), 1, 1).is_err());
    assert_eq!(p, before);
    assert!(
        p.stage_with_limit(&frozen.parts[1], &ctx(), 1, 1024 * 1024)
            .unwrap()
            .is_none()
    );
    let partial = p.clone();
    let forged = DeliveryUnit {
        index: 0,
        through: Some(4),
        changes: frozen.parts[1].changes.clone(),
    };
    assert!(p.commit(&forged, &ctx(), 1).is_err());
    assert_eq!(p, partial);
    let mut bad = frozen.parts[0].clone();
    bad.changes[0] = AuthorityChange::Remove {
        key: RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"a"}),
        },
        cursor: 4,
    };
    assert!(p.stage_with_limit(&bad, &ctx(), 1, 1024 * 1024).is_err());
    assert_eq!(p, partial);
    let complete = p
        .stage_with_limit(&frozen.parts[0], &ctx(), 1, 1024 * 1024)
        .unwrap()
        .unwrap();
    validate_delivery(&frozen.header, std::slice::from_ref(&complete)).unwrap();
    assert_eq!(
        p.stage_with_limit(&frozen.parts[0], &ctx(), 1, 1024 * 1024)
            .unwrap(),
        Some(complete.clone())
    );
    p.commit(&complete, &ctx(), 1).unwrap();
    assert_eq!(p.covered, Some(4));
    assert!(p.commit(&complete, &ctx(), 1).is_err());
    for variant in 0..4 {
        let mut h = frozen.header.clone();
        match variant {
            0 => h.through = 5,
            1 => h.context.stream = "User:b".into(),
            2 => h.units[0].parts.pop().map(|_| ()).unwrap(),
            _ => h.digest = "0".repeat(64),
        }
        assert!(validate_delivery(&h, std::slice::from_ref(&complete)).is_err());
    }
    let mut bad = serde_json::to_value(&frozen.parts[0]).unwrap();
    bad["changes"][0]["key"]["extra"] = json!(true);
    rejects::<DeliveryPart>(bad);
    let mut bad = serde_json::to_value(&frozen.header).unwrap();
    bad["units"][0]["extra"] = json!(true);
    rejects::<DeliveryHeader>(bad);
}
#[test]
fn malformed_coverage_and_rangeless_headers_are_rejected() {
    let f = frozen();
    let units = vec![DeliveryUnit {
        index: 0,
        through: None,
        changes: f.parts.iter().flat_map(|p| p.changes.clone()).collect(),
    }];
    assert!(
        freeze_delivery(
            ctx(),
            "p".into(),
            DeliveryPurpose::Sync,
            0,
            4,
            4,
            100,
            units,
            1
        )
        .is_err()
    );
    // Every plan is a Stream range: a rangeless or owner-carrying header
    // from an earlier protocol is not a delivery.
    for (field, value) in [
        ("after", Value::Null),
        ("through", Value::Null),
        (
            "owner",
            json!({"kind":"settlement","batchId":1,"mutationId":1}),
        ),
    ] {
        let mut bad = serde_json::to_value(&f.header).unwrap();
        bad[field] = value;
        rejects::<DeliveryHeader>(bad);
    }
}
#[test]
fn read_completions_require_definitive_outcome_and_empty_error_records() {
    let error = wire_form(
        json!({"context":ctx(),"requestId":"q","outcome":{"kind":"failed","code":"query.not_allowed","message":null},"records":[]}),
    );
    assert!(decode::<ReadResponse>(&serde_json::to_vec(&error).unwrap()).is_ok());
    let mut bad = error;
    bad["records"] =
        json!([{"key":{"model":"Entry","identity":{"id":"e"}},"cursor":null,"state":{}}]);
    rejects::<ReadResponse>(bad);
}
#[test]
fn accepted_acknowledgement_requires_every_server_model_input_target() {
    let mut r = request();
    r.mutations[0].operations.push(serde_json::from_value(json!({"step":3,"inputPath":"entry","operation":"create","model":"Entry","identity":{"id":"e"},"value":{"text":"A"}})).unwrap());
    r.digest = batch_digest(&r).unwrap();
    assert!(
        validate_acknowledgement(&r, &ack(&r), |_, _| true).is_err(),
        "missing owned target was accepted"
    );
    // A Model whose targets the client does not require needs none.
    validate_acknowledgement(&r, &ack(&r), |id, model| !(id == 1 && model == "Entry")).unwrap();
}
#[test]
fn handshake_and_reads_correlate_store_stream_request_and_modes() {
    // A 0.5 peer gets unsupported_protocol, not a digest or route error.
    assert!(
        decode::<HandshakeRequest>(br#"{"protocol":5,"storeId":"store","stream":"User:a"}"#)
            .is_err()
    );
    let request =
        decode::<HandshakeRequest>(br#"{"protocol":6,"storeId":"store","stream":"User:a"}"#)
            .unwrap();
    let response = decode::<HandshakeResponse>(
        br#"{"protocol":6,"storeId":"store","stream":"User:a","head":0,"projectionGeneration":"1"}"#,
    )
    .unwrap();
    response.admit(&request).unwrap();
    let mut other = response.clone();
    other.stream = "User:b".into();
    assert!(other.admit(&request).is_err());
    let mut value = wire_form(
        json!({"context":ctx(),"requestId":"fetch1","invocation":{"kind":"fetch","key":{"model":"Entry","identity":{"id":"e"}},"version":1}}),
    );
    let default = decode::<ReadRequest>(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(default.store);
    value["store"] = json!(true);
    assert_eq!(
        decode::<ReadRequest>(&serde_json::to_vec(&value).unwrap()).unwrap(),
        default
    );
    value["store"] = json!(false);
    let snapshot = decode::<ReadRequest>(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(!snapshot.store);
    let response=decode::<ReadResponse>(&serde_json::to_vec(&wire_form(json!({"context":ctx(),"requestId":"fetch1","outcome":{"kind":"succeeded","result":{"id":"e","text":"a"}},"records":[{"key":{"model":"Entry","identity":{"id":"e"}},"cursor":null,"state":{"text":"a"}}]}))).unwrap()).unwrap();
    response.admit(&snapshot, &ctx()).unwrap();
    let mut wrong = response.clone();
    wrong.request_id = "other".into();
    assert!(wrong.admit(&snapshot, &ctx()).is_err());
    let mut wrong = response;
    wrong.outcome = ReadOutcome::Succeeded {
        result: json!({"id":"e","text":"changed"}),
    };
    assert!(wrong.admit(&snapshot, &ctx()).is_err());
    value["invocation"]["extra"] = json!(true);
    rejects::<ReadRequest>(value);
    let delta = decode::<DeltaRequest>(
        &serde_json::to_vec(&wire_form(json!({"context":ctx(),"after":0,"through":4}))).unwrap(),
    )
    .unwrap();
    assert!(!delta.bootstrap);
    let f = frozen();
    let response = DeliveryResponse {
        header: f.header,
        parts: f.parts,
    };
    response.admit(&delta, &ctx()).unwrap();
    let mut wrong = delta;
    wrong.bootstrap = true;
    assert!(response.admit(&wrong, &ctx()).is_err());
}
#[test]
fn later_manifest_minimum_cannot_be_hidden_behind_earlier_coverage() {
    let f = frozen();
    let mut header = f.header;
    header.units.push(header.units[0].clone());
    header.units[0].through = Some(3);
    header.units[1].minimum_cursor = Some(2);
    header.digest = delivery_digest(&header).unwrap();
    assert!(header.validate().is_err());
}

#[test]
fn shared_wire_fixture_roundtrips_every_message_and_hash_domain() {
    let request = request();
    let acknowledgement = ack(&request);
    let f = frozen();
    let key = RecordKey {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
    };
    let query = ReadRequest {
        context: ctx(),
        request_id: "q1".into(),
        store: true,
        invocation: ReadInvocation::Query {
            name: "ReadEntries".into(),
            version: 1,
            args: json!({"limit":10}),
        },
    };
    let fetch = ReadRequest {
        context: ctx(),
        request_id: "f1".into(),
        store: false,
        invocation: ReadInvocation::Fetch {
            key: key.clone(),
            version: 1,
        },
    };
    let snapshot = ReadRecord {
        key: key.clone(),
        cursor: (),
        state: json!({"text":"accepted"}),
    };
    let success = ReadResponse {
        context: ctx(),
        request_id: "f1".into(),
        outcome: ReadOutcome::Succeeded {
            result: json!({"id":"e","text":"accepted"}),
        },
        records: vec![snapshot.clone()],
    };
    let failure = ReadResponse {
        context: ctx(),
        request_id: "q1".into(),
        outcome: ReadOutcome::Failed {
            code: "query.not_allowed".into(),
            message: None,
        },
        records: vec![],
    };
    let mut rejected = acknowledgement.clone();
    rejected.results[0].outcome = MutationOutcome::Rejected {
        code: "write.not_allowed".into(),
        message: Some("refused".into()),
    };
    let fixtures = json!({
        "mutationRequest":request,"batchAcknowledgement":acknowledgement,"rejectedAcknowledgement":rejected,
        "handshakeRequest":HandshakeRequest{protocol:PROTOCOL,store_id:"store".into(),stream:"User:a".into()},
        "handshakeResponse":HandshakeResponse{protocol:PROTOCOL,store_id:"store".into(),stream:"User:a".into(),head:40,projection_generation:"1".into()},
        "queryRequest":query,"fetchRequest":fetch,"readSuccess":success,"readFailure":failure,
        "readRecord":snapshot,"streamTarget":SettlementTarget::Stream{key:snapshot.key.clone(),cursor:20,fallback:snapshot.clone()},
        "privateTarget":SettlementTarget::Private{record:snapshot},
        "deltaRequest":DeltaRequest{context:ctx(),after:0,through:4,bootstrap:false},
        "cursorPageRequest":DeltaRequest{context:ctx(),after:2,through:4,bootstrap:false},
        "deliveryHeader":f.header,"deliveryUnit":DeliveryUnit{index:0,through:Some(4),changes:f.parts.iter().flat_map(|p|p.changes.clone()).collect()},
        "deliveryPart":f.parts[0],"deliveryResponse":DeliveryResponse{header:f.header.clone(),parts:f.parts.clone()},
        "removeChange":AuthorityChange::Remove{key:RecordKey{model:"Entry".into(),identity:json!({"id":"removed"})},cursor:30},
        "deliveryProgress":DeliveryProgress::new(&f.header,&ctx(),0).unwrap(),
        "cases":{"bootstrapRecordMovesPastStart":{"startCursor":40,"bootstrapCursor":null,"before":[{"model":"Entry","id":"e","cursor":20,"text":"a"}],"atPlan":[{"model":"Entry","id":"e","cursor":45,"text":"b"}],"observedHead":45,"expected":{"bootstrapCursor":40,"entryText":"b","entryCursor":45}}}
    });
    // AXTON_UPDATE_FIXTURES=1 regenerates the file after an intended wire change.
    if std::env::var_os("AXTON_UPDATE_FIXTURES").is_some() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/protocol/wire.json"
        );
        std::fs::write(
            path,
            serde_json::to_string_pretty(&fixtures).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    let saved: Value =
        serde_json::from_str(include_str!("../../../fixtures/protocol/wire.json")).unwrap();
    assert_eq!(saved, fixtures, "frozen cross-language fixture drift");
    macro_rules! check {
        ($name:literal,$ty:ty) => {{
            let value = decode::<$ty>(&serde_json::to_vec(&saved[$name]).unwrap()).unwrap();
            assert_eq!(serde_json::to_value(value).unwrap(), saved[$name]);
        }};
    }
    check!("mutationRequest", MutationRequest);
    check!("batchAcknowledgement", BatchAcknowledgement);
    check!("rejectedAcknowledgement", BatchAcknowledgement);
    check!("handshakeRequest", HandshakeRequest);
    check!("handshakeResponse", HandshakeResponse);
    check!("queryRequest", ReadRequest);
    check!("fetchRequest", ReadRequest);
    check!("readSuccess", ReadResponse);
    check!("readFailure", ReadResponse);
    check!("readRecord", ReadRecord);
    check!("streamTarget", SettlementTarget);
    check!("privateTarget", SettlementTarget);
    check!("deltaRequest", DeltaRequest);
    check!("cursorPageRequest", DeltaRequest);
    check!("deliveryHeader", DeliveryHeader);
    check!("deliveryUnit", DeliveryUnit);
    check!("deliveryPart", DeliveryPart);
    check!("deliveryResponse", DeliveryResponse);
    check!("removeChange", AuthorityChange);
    check!("deliveryProgress", DeliveryProgress);
    assert_eq!(
        batch_digest(&request).unwrap(),
        saved["mutationRequest"]["digest"]
    );
}
#[test]
fn receipt_stream_target_cannot_exceed_its_fenced_sync_head() {
    let r = request();
    let mut a = ack(&r);
    a.results[0].outcome = MutationOutcome::Accepted {
        sync_cursor: 10,
        materialization: "schema1".into(),
        result: Value::Null,
        targets: vec![SettlementTarget::Stream {
            key: RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"e"}),
            },
            cursor: 11,
            fallback: ReadRecord {
                key: RecordKey {
                    model: "Entry".into(),
                    identity: json!({"id":"e"}),
                },
                cursor: (),
                state: Value::Null,
            },
        }],
    };
    assert!(validate_acknowledgement(&r, &a, |_, _| true).is_err());
}

#[test]
fn removed_server_continuation_fields_are_rejected() {
    let mut request = serde_json::to_value(DeltaRequest {
        context: ctx(),
        after: 0,
        through: 4,
        bootstrap: false,
    })
    .unwrap();
    request["continuation"] = json!({"planId":"old","digest":"0".repeat(64),"unit":0,"part":0});
    assert!(decode::<DeltaRequest>(&serde_json::to_vec(&request).unwrap()).is_err());
}

#[test]
fn retained_mutation_descriptor_independent_digest_vector() {
    let mut d: axton_core::ActionDescriptor = serde_json::from_value(
        serde_json::json!({"name":"Void","version":1,"inputs":[],"outputs":[]}),
    )
    .unwrap();
    assert_eq!(
        axton_protocols::wire::mutation_descriptor_digest(&d).unwrap(),
        "aeb5c299c83ce9990ca03ab9f3928c7cc0d821949b510ade6d7144d936c8ff3b"
    );
    d.version = 2;
    assert_ne!(
        axton_protocols::wire::mutation_descriptor_digest(&d).unwrap(),
        "aeb5c299c83ce9990ca03ab9f3928c7cc0d821949b510ade6d7144d936c8ff3b"
    );
}
#[test]
fn materialization_preserves_read_semantics_with_distinct_domain() {
    let mut s: axton_core::Schema =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let a = axton_protocols::wire::materialization_id(&s, "1").unwrap();
    assert_eq!(a.len(), 64); // the materialization hash domain has independent fixture vectors above
    s.models[0].fields.reverse();
    assert_eq!(
        a,
        axton_protocols::wire::materialization_id(&s, "1").unwrap()
    );
    assert_ne!(
        a,
        axton_protocols::wire::materialization_id(&s, "2").unwrap()
    );
    s.models[0].bootstrap = !s.models[0].bootstrap;
    assert_ne!(
        a,
        axton_protocols::wire::materialization_id(&s, "1").unwrap()
    );
}
#[test]
fn batch_digest_excludes_the_materialization() {
    // The materialization is transport: it shapes the receipt, not the intent.
    let request = request();
    let mut moved = request.clone();
    moved.context.materialization = "schema2".into();
    assert_eq!(batch_digest(&moved).unwrap(), request.digest);
    validate_batch(&moved).unwrap();
    assert_ne!(
        request.digest, "2ecc56bed16fa22a88b260c91ad7e226998e86c8c5f97202afe460b7f923198c",
        "the axton:batch-intent:1 digest of this request"
    );
}
