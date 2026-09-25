use axton_compiler::compile;
use axton_compiler::validate::{
    ActionInput, ActionOutputSource, ActionOutputType, Cardinality, FieldType, Scalar,
};
use axton_compiler::{parse, validate};

#[test]
fn generated_actions_bind_to_shared_runtime_and_backend() {
    let descriptor = compile("model Todo { id String at DateTime @@id(id) } action Touch(todo Todo.update<at>, when DateTime) { echoed DateTime }").unwrap();
    let model = axton_compiler::typescript(&descriptor);
    let client = axton_compiler::client_typescript(&descriptor, "@axton/client");
    let backend = axton_compiler::backend_typescript(&descriptor, "@axton/server");
    assert!(model.contains("import type { ActionCall, ActionOptions } from './client.ts'"));
    assert!(client.contains("type ActionOutcome"));
    assert!(client.contains("readonly actions:"));
    assert!(model.contains("invokeAction"));
    assert!(backend.contains("export function createBackend"));
    assert!(backend.contains("ActionContext<Tx>"));
}

#[test]
fn retained_loader_identity_uses_its_own_datetime_contract() {
    let mut descriptor = compile("model Moment { at DateTime @@id(at) @@version(2) }").unwrap();
    let mut old = descriptor["schema"]["models"][0].clone();
    old["version"] = serde_json::json!(1);
    descriptor["backendModels"] =
        serde_json::json!([old, descriptor["schema"]["models"][0].clone()]);
    let backend = axton_compiler::backend_typescript(&descriptor, "@axton/server");
    assert!(backend.contains("v1(call: LoaderCall<Tx, MomentV1Identity>)"));
}

#[test]
fn action_values_and_explicit_outputs_are_typed() {
    let schema = parse("enum Status { active closed } model Todo { id String @@id(id) } action Search(query String?, labels String[]) { count Int status Status? statuses Status[] }").unwrap();
    let action = &validate(&schema).unwrap().actions[0];
    assert_eq!(
        action.inputs[0],
        ActionInput::Value {
            name: "query".into(),
            ty: FieldType::Scalar(Scalar::String),
            nullable: true,
            list: false
        }
    );
    assert_eq!(
        action.inputs[1],
        ActionInput::Value {
            name: "labels".into(),
            ty: FieldType::Scalar(Scalar::String),
            nullable: false,
            list: true
        }
    );
    assert_eq!(
        action
            .outputs
            .iter()
            .map(|x| x.cardinality)
            .collect::<Vec<_>>(),
        [
            Cardinality::Single,
            Cardinality::Optional,
            Cardinality::List
        ]
    );
    assert!(
        action
            .outputs
            .iter()
            .all(|x| x.source == ActionOutputSource::HandlerValue)
    );
    assert_eq!(
        action.outputs[1].ty,
        ActionOutputType::Value(FieldType::Enum("Status".into()))
    );
}

#[test]
fn action_model_operands_imply_bound_outputs() {
    let schema = parse("model Todo { id String title String @@id(id) } action Edit(one Todo.create, maybe Todo.update<title>?, many Todo.delete[]) { related Todo? }").unwrap();
    let action = &validate(&schema).unwrap().actions[0];
    assert_eq!(
        action
            .outputs
            .iter()
            .map(|x| x.cardinality)
            .collect::<Vec<_>>(),
        [
            Cardinality::Single,
            Cardinality::Optional,
            Cardinality::List,
            Cardinality::Optional
        ]
    );
    assert_eq!(action.outputs[0].ty, ActionOutputType::Model("Todo".into()));
    assert_eq!(
        action.outputs[2].ty,
        ActionOutputType::DeleteIdentity("Todo".into())
    );
    assert_eq!(
        action.outputs[1].source,
        ActionOutputSource::InputIdentity {
            input: "maybe".into()
        }
    );
    assert_eq!(
        action.outputs[3].source,
        ActionOutputSource::HandlerModelIdentity
    );
    assert_eq!(action.outputs[0].model_read_version, Some(1));
}

#[test]
fn action_model_operand_deprecation_is_rejected_at_the_operand() {
    let source = "model Todo { id String @@id(id) }\naction Edit(todo Todo.update @deprecated(reason: \"use other\"))";
    let err = validate(&parse(source).unwrap()).unwrap_err();
    assert!(err.starts_with("2:"), "{err}");
    assert!(err.contains("todo") && err.contains("deprecated"), "{err}");
}

#[test]
fn action_void_and_explicit_model_outputs_keep_their_shapes() {
    let schema = parse("model Todo { id String @@id(id) @@version(3) } action Void() action Load() { one Todo maybe Todo? many Todo[] }").unwrap();
    let valid = validate(&schema).unwrap();
    assert!(valid.actions[0].outputs.is_empty());
    let outputs = &valid.actions[1].outputs;
    assert_eq!(
        outputs.iter().map(|x| x.cardinality).collect::<Vec<_>>(),
        [
            Cardinality::Single,
            Cardinality::Optional,
            Cardinality::List
        ]
    );
    assert!(
        outputs
            .iter()
            .all(|x| x.ty == ActionOutputType::Model("Todo".into())
                && x.source == ActionOutputSource::HandlerModelIdentity
                && x.model_read_version == Some(3))
    );
}

#[test]
fn action_model_operations_keep_each_operand_cardinality() {
    for operation in ["create", "update", "delete"] {
        for (suffix, expected) in [
            ("", Cardinality::Single),
            ("?", Cardinality::Optional),
            ("[]", Cardinality::List),
        ] {
            let source = format!(
                "model Todo {{ id String title String @@id(id) }} action Do(todo Todo.{operation}{suffix})"
            );
            let valid = validate(&parse(&source).unwrap()).unwrap();
            let action = &valid.actions[0];
            let ActionInput::Model { slot } = &action.inputs[0] else {
                panic!("expected Model operand")
            };
            assert_eq!(slot.cardinality, expected, "{source}");
            assert_eq!(action.outputs[0].cardinality, expected, "{source}");
            assert_eq!(
                action.outputs[0].source,
                ActionOutputSource::InputIdentity {
                    input: "todo".into()
                },
                "{source}"
            );
            assert_eq!(
                action.outputs[0].ty,
                if operation == "delete" {
                    ActionOutputType::DeleteIdentity("Todo".into())
                } else {
                    ActionOutputType::Model("Todo".into())
                },
                "{source}"
            );
        }
    }
}

#[test]
fn action_sequence_can_match_all_prior_instances_or_a_list_target() {
    let source = "model Todo { id String @@id(id) } @sequence(after: [Prior()]) action Any(todo Todo.update) @sequence(after: [Prior(todos: todo)]) action One(todo Todo.update) action Prior(todos Todo.create[])";
    let valid = validate(&parse(source).unwrap()).unwrap();
    assert!(
        valid.actions[0].sequence.as_ref().unwrap().after[0]
            .bindings
            .is_empty()
    );
    assert_eq!(
        valid.actions[1].sequence.as_ref().unwrap().after[0].bindings[0].path,
        ["todo"]
    );
}

#[test]
fn action_sequence_unknown_target_names_the_target() {
    let source = "model Todo { id String @@id(id) }\n@sequence(after: [Missing()]) action Later(todo Todo.create)";
    let err = validate(&parse(source).unwrap()).unwrap_err();
    assert!(err.starts_with("2:"), "{err}");
    assert!(err.contains("Missing"), "{err}");
}

#[test]
fn action_semantic_errors_name_the_member_and_location() {
    for (source, name) in [
        ("action Search(query Object)", "Object"),
        (
            "model Todo { id String @@id(id) } action A(todo Todo.create) { todo Todo }",
            "todo",
        ),
        ("action A(x String, x Int)", "x"),
        ("action Call(x String)", "Call"),
        (
            "model Todo { id String @@id(id) } action Todo(x String)",
            "Todo",
        ),
        (
            "model Todo { id String @@id(id) } action Save(x String) action save(y String)",
            "save",
        ),
        (
            "model Todo { id String @@id(id) } action A(todo Todo.update<missing>)",
            "missing",
        ),
    ] {
        let err = validate(&parse(source).unwrap()).unwrap_err();
        assert!(err.contains("1:") && err.contains(name), "{source}: {err}");
    }
}

#[test]
fn action_preserves_restricted_patch_bindings_and_sequence() {
    let schema = parse(r#"
prerequisite Uploaded(key String)
model Parent { id String children Child[] @@id(id) }
model Child { id String parentId String title String @requires(Uploaded(key: self)) parent Parent @reference(via: [parentId]) @@id(id) }
@sequence(after: [Rename(child: child)])
action Add(parent Parent.create, child Child.create(parent: parent))
action Rename(child Child.update<title>)
"#).unwrap();
    let valid = validate(&schema).unwrap();
    let ActionInput::Model { slot: child } = &valid.actions[0].inputs[1] else {
        panic!("model input")
    };
    assert_eq!(child.bindings[0].slot, "parent");
    assert_eq!(
        valid.actions[0].sequence.as_ref().unwrap().after[0].bindings[0].path,
        ["child"]
    );
    let ActionInput::Model { slot: patch } = &valid.actions[1].inputs[0] else {
        panic!("model input")
    };
    assert_eq!(patch.allowed_patch_fields.as_ref().unwrap(), &["title"]);
    assert_eq!(valid.requirements[0].prerequisite, "Uploaded");
}

#[test]
fn action_value_input_can_share_a_name_with_an_explicit_output() {
    let schema =
        parse("model Todo { id String @@id(id) } action Echo(value String) { value String }")
            .unwrap();
    let action = &validate(&schema).unwrap().actions[0];
    assert_eq!(action.inputs.len(), 1);
    assert_eq!(action.outputs.len(), 1);
}
#[test]
fn schema_and_mutations() {
    let v=compile("enum Status { active archived } model Entry { owner UUID id UUID title String note String? labels String[] at DateTime status Status @@id(owner,id) @@unique(title) } mutation Edit { entry Entry.update<title,note> @@version(2) }").unwrap();
    assert_eq!(
        v["schema"]["models"][0]["identity"],
        serde_json::json!(["owner", "id"])
    );
    assert_eq!(v["mutations"][0]["version"], 2);
    assert_eq!(
        v["mutations"][0]["slots"][0]["allowedPatchFields"],
        serde_json::json!(["title", "note"])
    );
}
#[test]
fn rejects_unknown_with_location() {
    let e = compile("model A { id UUID @@id(id) }\nunknown Stuff {}").unwrap_err();
    assert!(e.contains("2:"), "{e}");
}
#[test]
fn rejects_invalid_identity() {
    assert!(compile("model A { id UUID? @@id(id) }").is_err());
}
#[test]
fn emitters_include_typed_conversion() {
    let v = compile(include_str!("../../../fixtures/compiler/example.model")).unwrap();
    let ts = axton_compiler::typescript(&v);
    assert!(ts.contains("export interface EntryIdentity"));
    assert!(ts.contains("new Date("));
    assert!(ts.contains("EditEntry"));
    let dart = axton_compiler::dart(&v);
    assert!(dart.contains("class EntryPatch"));
    assert!(dart.contains("DateTime.parse("));
}
#[test]
fn relationships_bindings_and_dependency_metadata() {
    let v=compile(r#"prerequisite Uploaded(key String)
 model Parent { id UUID children Child[] @@id(id) }
 model Child { id UUID parentId UUID label String @requires(Uploaded(key: self)) parent Parent @reference(via: [parentId], onTargetDelete: delete) @@id(id) }
 mutation Add { parent Parent.create children Child.create(parent: parent)[] @@sequence(after: [Rename(parent: parent)]) }
 mutation Rename { parent Parent.update<> }
 "#).unwrap();
    assert_eq!(
        v["schema"]["models"][1]["relations"][0]["fields"],
        serde_json::json!(["parentId"])
    );
    assert_eq!(
        v["mutations"][0]["slots"][1]["bindings"][0]["slot"],
        "parent"
    );
    assert_eq!(v["prerequisites"][0]["name"], "Uploaded");
    // `Parent.update<>` is valid: an empty patch is a no-op update ([#49](https://github.com/zanminwang/axton/issues/49)).
    assert_eq!(v["mutations"][1]["name"], "Rename");
    assert_eq!(
        v["mutations"][1]["slots"][0]["allowedPatchFields"],
        serde_json::json!([])
    );
}
#[test]
fn rejects_dependency_typos() {
    assert!(compile("prerequisite Exists(key String) model A { id UUID label String @requires(Missing(key: self)) @@id(id) }").is_err());
    assert!(
        compile(
            "model A { id UUID @@id(id) } mutation Add { a A.create @@version(2) @@version(3) }"
        )
        .is_err()
    );
}
#[test]
fn singular_inverse_requires_a_unique_foreign_key() {
    assert!(compile("model Parent { id String child Child? @@id(id) } model Child { id String parentId String parent Parent @reference(via:[parentId]) @@id(id) }").is_err());
    assert!(compile("model Parent { id String child Child? @@id(id) } model Child { id String parentId String parent Parent @reference(via:[parentId]) @@id(id) @@unique(parentId) }").is_ok());
}
#[test]
fn backend_emitter_declares_handlers_loaders_and_references() {
    let v = compile(include_str!("../../../fixtures/compiler/relations.model")).unwrap();
    let ts = axton_compiler::backend_typescript(&v, "@axton/server");
    assert!(ts.contains("from \"@axton/server\""));
    assert!(ts.contains("export interface Handlers<Tx> {"));
    assert!(ts.contains(
        " addBook: { v1(call: HandlerCall<Tx, AddBookInput>): Promise<void> } | ((call: HandlerCall<Tx, AddBookInput>) => Promise<void>);"
    ));
    assert!(ts.contains(
        " addComment: { v1(call: HandlerCall<Tx, AddCommentInput>): Promise<void> } | ((call: HandlerCall<Tx, AddCommentInput>) => Promise<void>);"
    ));
    assert!(ts.contains("export interface Loaders<Tx> {"));
    assert!(
        ts.contains(
            " book: { v1(call: LoaderCall<Tx, BookIdentity>): Promise<readonly (Book | null)[]> } | ((call: LoaderCall<Tx, BookIdentity>) => Promise<readonly (Book | null)[]>);"
        )
    );
    assert!(ts.contains("export function Book(identity: BookIdentity): RecordRef { return { model: \"Book\", identity: encodeBookIdentity(identity) }; }"));
    assert!(ts.contains("export interface AddBookInput {\n book: Book;\n}"));
    assert!(ts.contains("export function createBackend<Tx>("));
    assert!(!axton_compiler::typescript(&v).contains("backendConfig"));
}
#[test]
fn backend_emitter_groups_handler_versions_under_the_mutation_name() {
    let v = compile("model A { id String title String @@id(id) } mutation Edit { a A.update<title> @@version(2) }").unwrap();
    let mut with_history = v.clone();
    let mut old = v["mutations"][0].clone();
    old["version"] = serde_json::json!(1);
    with_history["backendMutations"] = serde_json::json!([old, v["mutations"][0].clone()]);
    let ts = axton_compiler::backend_typescript(&with_history, "@axton/server");
    assert!(
        ts.contains(
            " edit: { v1(call: HandlerCall<Tx, EditV1Input>): Promise<void>; v2(call: HandlerCall<Tx, EditInput>): Promise<void> };\n"
        ),
        "{ts}"
    );
    assert!(!ts.contains(" editV1(call:"));
}

#[test]
fn backend_emitter_accepts_a_bare_function_only_for_a_v1_only_mutation() {
    let v = compile("model A { id String title String @@id(id) } mutation Save { a A.create }")
        .unwrap();
    let ts = axton_compiler::backend_typescript(&v, "@axton/server");
    assert!(
        ts.contains(
            " save: { v1(call: HandlerCall<Tx, SaveInput>): Promise<void> } | ((call: HandlerCall<Tx, SaveInput>) => Promise<void>);\n"
        ),
        "{ts}"
    );
    let later = compile(
        "model A { id String title String @@id(id) } mutation Save { a A.create @@version(2) }",
    )
    .unwrap();
    let ts = axton_compiler::backend_typescript(&later, "@axton/server");
    assert!(
        ts.contains(" save: { v2(call: HandlerCall<Tx, SaveInput>): Promise<void> };\n"),
        "{ts}"
    );
    assert!(
        !ts.contains("| ((call: HandlerCall"),
        "a single non-v1 version has no shorthand: {ts}"
    );
}

#[test]
fn generated_clients_expose_one_server_connection() {
    let schema = compile("model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> }").unwrap();
    let ts = axton_compiler::client_typescript(&schema, "@example/custom-runtime");
    assert!(ts.contains("type ServerOptions"));
    assert!(ts.contains("server?: ServerOptions"));
    assert!(ts.contains("client.connect(options.server"));
    assert!(!ts.contains("LiveTransport"));
    assert!(!ts.contains("transport?:"));
    assert!(
        ts.find("connection options were removed").unwrap() < ts.find("await Client.open").unwrap()
    );
    let schema = compile("model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> }").unwrap();
    let dart = axton_compiler::dart(&schema);
    assert!(dart.contains("show RuntimeConnection, SyncServer"));
    assert!(dart.contains("SyncServer? server"));
    assert!(dart.contains("client.connect(server"));
    assert!(!dart.contains("LiveTransport"));
    assert!(!dart.contains("Transport? transport"));
}

/// The generated client is the whole client: typed models (with a per-record
/// sync state), top-level mutations and the runtime members, in both languages.
#[test]
fn generated_clients_are_the_whole_client() {
    let schema = compile("model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> }").unwrap();
    let ts = axton_compiler::client_typescript(&schema, "@example/custom-runtime");
    for member in [
        "readonly mutate: Mutate;",
        "this.mutate = new Mutate(client)",
        "syncState(): Promise<ClientSyncState>",
        "dismissRejection(ordinal: number)",
        "drop(ordinal: number)",
        "pendingTasks()",
        "setReadiness(key: string",
        "runPrerequisites(",
        "async connect(server: ServerOptions",
        "querySpec(model: string",
        "readSql(sql: string",
    ] {
        assert!(ts.contains(member), "missing {member}: {ts}");
    }
    assert!(!ts.contains("status()"), "{ts}");
    let schema = compile(
        "model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> } mutation Touch { entry Entry.update<title> }",
    )
    .unwrap();
    let model = axton_compiler::typescript(&schema);
    assert!(
        model.contains("export type MutationName = 'Edit'|'Touch';"),
        "{model}"
    );
    assert!(
        model.contains("async syncState(identity:EntryIdentity):Promise<SyncState>"),
        "{model}"
    );
    assert!(
        model.contains("export class Mutate { readonly port:MutatePort;"),
        "{model}"
    );
    assert!(
        model.contains(
            "interface WritePort extends ReadPort { direct(operation:object):Promise<void>; }"
        ),
        "transactions expose direct writes without mutation submission: {model}"
    );
    let write_port = model
        .lines()
        .find(|line| line.starts_with("export interface WritePort "))
        .unwrap();
    assert!(!write_port.contains("mutate"), "{write_port}");
    let dart = axton_compiler::dart(&schema);
    for member in [
        "late final Mutate mutate = Mutate(client);",
        "Future<Map<String,dynamic>> syncState() => client.syncState();",
        "Future<void> dismissRejection(int ordinal)",
        "Future<void> drop(int ordinal)",
        "Future<RuntimeConnection> connect(SyncServer server",
        "Future<SyncState> syncState(EntryIdentity identity)",
        "class Mutate { final MutatePort port;",
    ] {
        assert!(dart.contains(member), "missing {member}: {dart}");
    }
}

#[test]
fn generated_transaction_facades_are_local_only() {
    let schema = compile("model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> }").unwrap();
    let ts = axton_compiler::typescript(&schema);
    let ts_transaction = ts
        .lines()
        .find(|line| line.starts_with("export class GeneratedTransaction "))
        .unwrap();
    assert!(
        ts_transaction.contains("readonly models:TxModels;"),
        "{ts_transaction}"
    );
    assert!(!ts_transaction.contains("mutate"), "{ts_transaction}");
    assert!(
        ts.contains("export class Mutate { readonly port:MutatePort;"),
        "{ts}"
    );
    let ts_client = axton_compiler::client_typescript(&schema, "@example/custom-runtime");
    assert!(
        ts_client.contains("this.mutate = new Mutate(client)"),
        "{ts_client}"
    );

    let dart = axton_compiler::dart(&schema);
    let dart_transaction = dart
        .lines()
        .find(|line| line.starts_with("class GeneratedTransaction "))
        .unwrap();
    assert!(
        dart_transaction.contains("late final TxModels models"),
        "{dart_transaction}"
    );
    assert!(!dart_transaction.contains("mutate"), "{dart_transaction}");
    assert!(
        dart.contains("class Mutate { final MutatePort port;"),
        "{dart}"
    );
    assert!(
        dart.contains("late final Mutate mutate = Mutate(client);"),
        "{dart}"
    );
}

fn line_of(error: &str) -> usize {
    error
        .split(':')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no line in {error}"))
}

#[test]
fn semantic_errors_report_the_offending_declaration() {
    // Every case pads the input so end-of-file is far below the declaration.
    let pad = "\n\n\n\n// trailing comment\n";
    let cases: &[(&str, usize, &str)] = &[
        (
            "model Parent { id UUID @@id(id) }\nmodel Child {\n id UUID\n parent Parent @reference(via: [missing])\n @@id(id)\n}",
            4,
            "unknown reference field",
        ),
        (
            "model Parent { id UUID @@id(id) }\nmodel Child {\n id UUID\n parentId UUID\n parent Parent @reference(via: [parentId])\n twin Parent @reference(via: [parentId], onTargetDelete: cascade)\n @@id(id)\n}",
            6,
            "unsupported onTargetDelete",
        ),
        (
            "model Parent { id String child Child? @@id(id) }\nmodel Child {\n id String\n parentId String\n parent Parent @reference(via:[parentId])\n @@id(id)\n}",
            1,
            "singular inverse requires unique reference fields",
        ),
        (
            "model A {\n id UUID\n\n label Missing\n @@id(id)\n}",
            4,
            "unknown or unsupported field type",
        ),
        (
            "model Parent { id UUID @@id(id) }\nmodel Child { id UUID parentId UUID parent Parent @reference(via: [parentId]) @@id(id) }\nmutation Add {\n parent Parent.create\n child Child.create(parent: nobody)\n}",
            5,
            "unknown parent slot",
        ),
        (
            "model A { id UUID @@id(id) }\nmutation Add {\n a A.create\n @@sequence(after: [Missing(a: a)])\n}",
            4,
            "unknown sequence mutation",
        ),
        (
            "prerequisite Exists(key String)\nmodel A {\n id UUID\n label String @requires(Missing(key: self))\n @@id(id)\n}",
            4,
            "unknown prerequisite",
        ),
        (
            "model A { id UUID @@id(id) }\nmutation Add { a A.create }\n\nmutation Add { a A.create }",
            4,
            "duplicate mutation",
        ),
        (
            "model A { id UUID @@id(id) }\nmutation Edit {\n a A.update<nope>\n}",
            3,
            "invalid allowed patch field",
        ),
        (
            "model A {\n id UUID\n @@id(id)\n @@unique(missing)\n}",
            4,
            "invalid unique fields",
        ),
        (
            "model A { id UUID @@id(id) }\n\nmodel A { id UUID @@id(id) }",
            3,
            "duplicate",
        ),
        (
            "model A { id UUID @@id(id) }\n\nmodel B {\n id UUID\n}",
            3,
            "identity",
        ),
        (
            "model A { id UUID @@id(id) }\nmutation Remove {\n entries A.delete[]\n maybe A.delete?\n}",
            4,
            "ambiguous slot",
        ),
    ];
    for (source, line, message) in cases {
        let e = compile(&format!("{source}{pad}")).unwrap_err();
        assert!(e.contains(message), "expected {message:?} in {e:?}");
        assert_eq!(line_of(&e), *line, "{e}");
    }
}

#[test]
fn rejects_model_and_enum_names_the_generated_client_uses() {
    for name in [
        "SyncState",
        "Rejection",
        "Mutate",
        "GeneratedClient",
        "Actions",
        "DirectCalls",
        "ActionPort",
        "ActionContext",
        "ActionRejected",
        "Transaction",
        // The Scope facade and the handle types it re-exports
        // ([#150](https://github.com/zanminwang/axton/issues/150)).
        "Scopes",
        "Subscription",
        "SubscriptionStatus",
        "SubscriptionInitialization",
        "SubscriptionConnection",
        "SubscriptionClosedException",
    ] {
        let e = compile(&format!("model {name} {{ id UUID @@id(id) }}")).unwrap_err();
        assert!(e.contains("generated client"), "{name}: {e}");
        let e = compile(&format!(
            "enum {name} {{ a b }}\nmodel Other {{ id UUID @@id(id) }}"
        ))
        .unwrap_err();
        assert!(e.contains("generated client"), "enum {name}: {e}");
    }
    for name in [
        "Status",
        "SyncStates",
        "Rejections",
        "Order",
        "Scope",
        "Subscriptions",
    ] {
        assert!(
            compile(&format!("model {name} {{ id UUID @@id(id) }}")).is_ok(),
            "{name} should stay valid"
        );
    }
}

#[test]
fn rejects_reserved_model_names_at_the_declaration() {
    for name in ["sqlite_entry", "SQLite_Entry", "axton_entry", "AXTON_entry"] {
        let e = compile(&format!(
            "model Other {{ id UUID @@id(id) }}\n\nmodel {name} {{ id UUID @@id(id) }}\n\n\n"
        ))
        .unwrap_err();
        assert!(e.contains("reserved"), "{name}: {e}");
        assert_eq!(line_of(&e), 3, "{name}: {e}");
    }
    for name in ["Sqlite", "sqlitex", "my_sqlite_table", "axtonEntry"] {
        assert!(
            compile(&format!("model {name} {{ id UUID @@id(id) }}")).is_ok(),
            "{name} should stay valid"
        );
    }
}

#[test]
fn structural_refusals_a_schema_author_is_likely_to_hit() {
    // Each case names the rule, the input that breaks it and a valid twin that
    // differs only in that rule, so the refusal is attributable to the rule alone.
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "duplicate field",
            "model A {\n id UUID\n text String\n text String\n @@id(id)\n}",
            "model A {\n id UUID\n text String\n note String\n @@id(id)\n}",
            "duplicate field",
        ),
        (
            "reference identity arity mismatch",
            "model Parent { id UUID kind String @@id(id, kind) }\nmodel Child {\n id UUID\n parentId UUID\n parent Parent @reference(via: [parentId])\n @@id(id)\n}",
            "model Parent { id UUID kind String @@id(id, kind) }\nmodel Child {\n id UUID\n parentId UUID\n parentKind String\n parent Parent @reference(via: [parentId, parentKind])\n @@id(id)\n}",
            "reference identity arity mismatch",
        ),
        (
            "reference field type mismatch",
            "model Parent { id UUID @@id(id) }\nmodel Child {\n id UUID\n parentId String\n parent Parent @reference(via: [parentId])\n @@id(id)\n}",
            "model Parent { id UUID @@id(id) }\nmodel Child {\n id UUID\n parentId UUID\n parent Parent @reference(via: [parentId])\n @@id(id)\n}",
            "reference field type mismatch",
        ),
        (
            "unknown reference argument",
            "model Parent { id UUID @@id(id) }\nmodel Child {\n id UUID\n parentId UUID\n parent Parent @reference(via: [parentId], cascade: true)\n @@id(id)\n}",
            "model Parent { id UUID @@id(id) }\nmodel Child {\n id UUID\n parentId UUID\n parent Parent @reference(via: [parentId], onTargetDelete: delete)\n @@id(id)\n}",
            "unknown reference argument",
        ),
        (
            "ambiguous inverse",
            "model Parent {\n id UUID\n children Child[]\n @@id(id)\n}\nmodel Child {\n id UUID\n parentId UUID\n otherId UUID\n parent Parent @reference(via: [parentId])\n other Parent @reference(via: [otherId])\n @@id(id)\n}",
            "model Parent {\n id UUID\n children Child[] @inverse(owner)\n @@id(id)\n}\nmodel Child {\n id UUID\n parentId UUID\n otherId UUID\n parent Parent @reference(owner, via: [parentId])\n other Parent @reference(via: [otherId])\n @@id(id)\n}",
            "inverse must resolve to exactly one reference",
        ),
        (
            "binding parent must be a single slot of the referenced model",
            "model Parent { id UUID @@id(id) }\nmodel Child { id UUID parentId UUID parent Parent @reference(via: [parentId]) @@id(id) }\nmutation Add {\n parents Parent.create[]\n child Child.create(parent: parents)\n}",
            "model Parent { id UUID @@id(id) }\nmodel Child { id UUID parentId UUID parent Parent @reference(via: [parentId]) @@id(id) }\nmutation Add {\n parent Parent.create\n child Child.create(parent: parent)\n}",
            "binding parent must be single matching model",
        ),
        (
            "sequence target model mismatch",
            "model A { id UUID @@id(id) }\nmodel B { id UUID @@id(id) }\nmutation First { a A.create }\nmutation Second {\n b B.create\n @@sequence(after: [First(a: b)])\n}",
            "model A { id UUID text String @@id(id) }\nmutation First { a A.create }\nmutation Second {\n a A.update<text>\n @@sequence(after: [First(a: a)])\n}",
            "sequence target model mismatch",
        ),
        (
            "list slot followed by an optional slot of the same model and operation",
            "model A { id UUID text String @@id(id) }\nmutation Remove {\n entries A.delete[]\n maybe A.delete?\n}",
            "model A { id UUID text String @@id(id) }\nmutation Remove {\n entries A.delete[]\n maybe A.update?\n}",
            "ambiguous slot",
        ),
        (
            "optional slot followed by a single slot of the same model and operation",
            "model A { id UUID text String @@id(id) }\nmutation Remove {\n first A.delete?\n second A.delete\n}",
            "model A { id UUID text String @@id(id) }\nmutation Remove {\n first A.delete\n second A.delete?\n}",
            "ambiguous slot",
        ),
        (
            "same model and operation separated only by an optional slot",
            "model A { id UUID text String @@id(id) }\nmodel B { id UUID @@id(id) }\nmutation Remove {\n first A.delete[]\n other B.create?\n second A.delete?\n}",
            "model A { id UUID text String @@id(id) }\nmodel B { id UUID @@id(id) }\nmutation Remove {\n first A.delete[]\n other B.create\n second A.delete?\n}",
            "ambiguous slot",
        ),
    ];
    for (rule, invalid, valid, message) in cases {
        let e = compile(invalid).unwrap_err();
        assert!(e.contains(message), "{rule}: expected {message:?} in {e:?}");
        assert!(
            compile(valid).is_ok(),
            "{rule}: the valid twin was refused: {:?}",
            compile(valid).err()
        );
    }
}

#[test]
fn model_versions_reach_every_generated_surface() {
    // The client's declared read contract is the `version` of each model in the
    // embedded schema; the backend additionally carries every retained contract.
    let v = compile("enum Status { open closed } model Task { id UUID status Status @@id(id) @@version(2) } model Note { id UUID @@id(id) }").unwrap();
    assert_eq!(v["schema"]["models"][0]["version"], 2);
    assert_eq!(
        v["schema"]["models"][1]["version"], 1,
        "omitted is version 1"
    );
    let ts = axton_compiler::typescript(&v);
    assert!(
        ts.contains(r#""name":"Task","relations":[],"unique":[],"version":2}"#),
        "{ts}"
    );
    assert!(
        ts.contains(r#""name":"Note","relations":[],"unique":[],"version":1}"#),
        "{ts}"
    );
    let dart = axton_compiler::dart(&v);
    assert!(
        dart.contains(r#""name":"Task","relations":[],"unique":[],"version":2}"#),
        "{dart}"
    );
    let mut with_history = v.clone();
    let old = serde_json::json!({"name":"Task","version":1,"identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"uuid"}}],"enums":[]});
    with_history["backendModels"] = serde_json::json!([old]);
    let backend = axton_compiler::backend_typescript(&with_history, "@axton/server");
    assert!(backend.contains(r#""models":[{"enums":[],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"uuid"}}],"identity":["id"],"name":"Task","version":1}]"#), "{backend}");
    assert!(!backend.contains("backendModels"), "{backend}");
}

#[test]
fn backend_emitter_groups_loader_versions_under_the_model_name() {
    let v = compile("enum Status { open closed archived } model Task { id UUID title String status Status @@id(id) @@version(2) } model Note { id UUID text String @@id(id) }").unwrap();
    let mut with_history = v.clone();
    // The retained v1 contract: no `title`, and `Status` as it was published.
    let old = serde_json::json!({"name":"Task","version":1,"identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"uuid"}},{"name":"status","nullable":false,"type":{"kind":"enum","name":"Status"}}],"enums":[{"name":"Status","values":["open","closed"]}]});
    let mut current = v["schema"]["models"][0].clone();
    current["enums"] = v["schema"]["enums"].clone();
    let note = serde_json::json!({"name":"Note","version":1,"identity":["id"],"fields":v["schema"]["models"][1]["fields"],"enums":[]});
    with_history["backendModels"] = serde_json::json!([note, old, current]);
    let ts = axton_compiler::backend_typescript(&with_history, "@axton/server");
    // An older contract is its own record type, with the enum values of its time inline.
    assert!(
        ts.contains(
            "export interface TaskV1 {\n id: string;\n status: \"open\" | \"closed\";\n}\n"
        ),
        "{ts}"
    );
    assert!(ts.contains("export type Task = TaskRecord;"), "{ts}");
    assert!(
        !ts.contains("TaskV2"),
        "the latest version keeps the plain name: {ts}"
    );
    assert!(
        ts.contains(" task: { v1(call: LoaderCall<Tx, TaskV1Identity>): Promise<readonly (TaskV1 | null)[]>; v2(call: LoaderCall<Tx, TaskIdentity>): Promise<readonly (Task | null)[]> };\n"),
        "{ts}"
    );
    assert!(
        ts.contains(" note: { v1(call: LoaderCall<Tx, NoteIdentity>): Promise<readonly (Note | null)[]> } | ((call: LoaderCall<Tx, NoteIdentity>) => Promise<readonly (Note | null)[]>);\n"),
        "{ts}"
    );
    // Without a history the schema's own version is the only retained one; a
    // single non-v1 version has no shorthand.
    let ts = axton_compiler::backend_typescript(&v, "@axton/server");
    assert!(
        ts.contains(" task: { v2(call: LoaderCall<Tx, TaskIdentity>): Promise<readonly (Task | null)[]> };\n"),
        "{ts}"
    );
    assert!(!ts.contains("TaskV1"), "{ts}");
}

#[test]
fn deprecations_reach_every_generated_surface_and_leave_the_descriptors_alone() {
    let v = compile("enum Status { active archived @deprecated(reason: \"use closed\") closed }\nmodel Task { id UUID name String @deprecated(reason: \"renamed to title\") title String legacy Int? @deprecated status Status @@id(id) }\nmutation Edit { task Task.update<title> old Task.update<name>? @deprecated(reason: \"use task\") }").unwrap();
    // The runtime descriptors do not carry deprecation: it is a generated-code notice only.
    assert!(
        !v["schema"].to_string().contains("deprecat"),
        "{}",
        v["schema"]
    );
    assert!(!v["mutations"].to_string().contains("deprecat"));
    assert_eq!(
        v["deprecations"],
        serde_json::json!([
            {"kind":"enumValue","enum":"Status","value":"archived","reason":"use closed"},
            {"kind":"field","model":"Task","field":"name","reason":"renamed to title"},
            {"kind":"field","model":"Task","field":"legacy","reason":null},
            {"kind":"slot","mutation":"Edit","slot":"old","reason":"use task"}
        ])
    );
    let ts = axton_compiler::typescript(&v);
    assert!(ts.contains("export interface Task {\n id: string;\n /** @deprecated renamed to title */\n name: string;\n title: string;\n /** @deprecated */\n legacy: number | null;\n"), "{ts}");
    assert!(
        ts.contains(
            "export interface TaskPatch {\n /** @deprecated renamed to title */\n name?: string;\n"
        ),
        "{ts}"
    );
    assert!(ts.contains("/** @deprecated \"archived\": use closed */\nexport type Status = \"active\" | \"archived\" | \"closed\";"), "{ts}");
    assert!(ts.contains("export interface EditArgs {\n task: { identity:TaskIdentity; values:Pick<TaskPatch, \"title\"> };\n /** @deprecated use task */\n old?: { identity:TaskIdentity; values:Pick<TaskPatch, \"name\"> };\n}"), "{ts}");
    let backend = axton_compiler::backend_typescript(&v, "@axton/server");
    assert!(backend.contains("export interface EditInput {\n task: { identity: TaskIdentity; patch: Pick<TaskPatch, \"title\"> };\n /** @deprecated use task */\n old: { identity: TaskIdentity; patch: Pick<TaskPatch, \"name\"> } | null;\n}"), "{backend}");
    let dart = axton_compiler::dart(&v);
    assert!(
        dart.contains("enum Status { active, @Deprecated('use closed') archived, closed }"),
        "{dart}"
    );
    assert!(dart.contains("class Task {\n final String id;\n @Deprecated('renamed to title')\n final String name;\n final String title;\n @Deprecated('')\n final int? legacy;\n"), "{dart}");
    assert!(
        dart.contains(
            "class TaskPatch {\n @Deprecated('renamed to title')\n final Present<String>? name;\n"
        ),
        "{dart}"
    );
    assert!(dart.contains("class TaskFilter {\n final Present<String>? id;\n @Deprecated('renamed to title')\n final Present<String>? name;\n"), "{dart}");
    assert!(dart.contains("Map<String,dynamic> edit({required EditTaskUpdate task,@Deprecated('use task') EditOldUpdate? old})"), "{dart}");
    assert!(dart.contains(" Future<int> edit({required EditTaskUpdate task,@Deprecated('use task') EditOldUpdate? old})"), "{dart}");
}
#[test]
fn action_descriptors_separate_values_operands_and_output_sources() {
    let source = "model Todo { id String title String @@id(id) } action Save(label String?, todo Todo.create, maybe Todo.update<title>?, gone Todo.delete[]) { related Todo? count Int }";
    let descriptor = axton_compiler::compile(source).unwrap();
    let action = &descriptor["actions"][0];
    assert_eq!(action["name"], "Save");
    assert_eq!(action["inputs"][0]["kind"], "value");
    assert_eq!(action["inputs"][0]["required"], true);
    assert_eq!(action["inputs"][0]["nullable"], true);
    assert_eq!(action["inputs"][1]["kind"], "model");
    assert_eq!(action["inputs"][2]["cardinality"], "optional");
    assert_eq!(
        action["inputs"][2]["allowedPatchFields"],
        serde_json::json!(["title"])
    );
    assert_eq!(
        action["outputs"][1]["source"],
        serde_json::json!({"inputIdentity":"maybe"})
    );
    assert_eq!(action["outputs"][2]["kind"], "deleteIdentity");
    assert_eq!(action["outputs"][2]["cardinality"], "list");
    assert_eq!(
        action["outputs"][3]["source"],
        serde_json::json!("handlerIdentity")
    );
    assert_eq!(
        action["outputs"][3]["handlerType"]["fields"][0]["name"],
        "id"
    );
    assert_eq!(action["outputs"][3]["modelReadVersion"], 1);
    assert!(
        descriptor["schema"]["clientPolicies"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn action_descriptors_keep_bindings_sequence_prerequisites_and_composite_keys() {
    let source = r#"
prerequisite Uploaded(key String)
model Parent { id String children Child[] @@id(id) }
model Child { tenant String id String parentId String title String @requires(Uploaded(key: self)) parent Parent @reference(via: [parentId]) @@id(tenant, id) }
@sequence(after: [Rename(child: child)])
action Add(parent Parent.create, child Child.create(parent: parent)) { found Child[] }
action Rename(child Child.update<title>)
"#;
    let descriptors = axton_compiler::compile(source).unwrap();
    let action = &descriptors["actions"][0];
    assert_eq!(action["inputs"][1]["bindings"][0]["slot"], "parent");
    assert_eq!(
        action["sequence"]["after"][0]["arguments"]["child"],
        "child"
    );
    assert_eq!(action["outputs"][2]["cardinality"], "list");
    assert_eq!(
        action["outputs"][2]["handlerType"]["fields"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        action["outputs"][2]["handlerType"]["fields"][0]["name"],
        "tenant"
    );
    let history = axton_compiler::reconcile_action_history(&descriptors, None).unwrap();
    let snapshot = &history["actions"]["Add"]["1"];
    assert_eq!(snapshot["requirements"][0]["name"], "Uploaded");
    assert_eq!(snapshot["prerequisites"][0]["name"], "Uploaded");
    assert_eq!(snapshot["sequence"], action["sequence"]);
}

#[test]
fn retained_generated_action_binding_validates_without_relation_snapshots() {
    let source = "model Parent { id String children Child[] @@id(id) } model Child { id String parentId String parent Parent @reference(via: [parentId]) @@id(id) } action Add(parent Parent.create, child Child.create(parent: parent))";
    let mut config = axton_compiler::compile(source).unwrap();
    let models = axton_compiler::reconcile_model_history(&config, None).unwrap();
    let retained_models: Vec<_> = models["models"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|versions| versions.as_object().unwrap().values().cloned())
        .collect();
    config["backendModels"] = serde_json::json!(retained_models);
    let history = axton_compiler::reconcile_action_history(&config, None).unwrap();
    let snapshot = history["actions"]["Add"]["1"].clone();
    assert!(
        snapshot["input"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model.get("relations").is_none())
    );
    let mut schema = config["schema"].clone();
    schema["actions"] = serde_json::json!([snapshot]);
    schema["resultModels"] = serde_json::json!(retained_models);
    let schema = axton_core::Schema::from_value(schema).unwrap();
    let action = schema.action("Add", 1).unwrap();
    assert!(
        axton_core::normalize_action_args(
            &schema,
            action,
            &serde_json::json!({"parent":{"id":"p"},"child":{"id":"c","parentId":"p"}})
        )
        .is_ok()
    );
    assert!(
        axton_core::normalize_action_args(
            &schema,
            action,
            &serde_json::json!({"parent":{"id":"p"},"child":{"id":"c","parentId":"other"}})
        )
        .is_err()
    );
}

#[test]
fn action_typescript_emits_flattened_operands_for_generated_client() {
    let v = compile("model Todo { id String title String @@id(id) } action AddTodo(todo Todo.create, patch Todo.update<title>?, gone Todo.delete[], label String?) { related Todo? matches Todo[] count Int }").unwrap();
    let ts = axton_compiler::typescript(&v);
    assert!(ts.contains("export type TodoCreate = Todo;"), "{ts}");
    assert!(ts.contains("export type TodoUpdate<K extends keyof TodoPatch = keyof TodoPatch> = TodoIdentity & Partial<Pick<TodoPatch, K>>;"), "{ts}");
    assert!(
        ts.contains("export type TodoDelete = TodoIdentity;"),
        "{ts}"
    );
    assert!(ts.contains("export interface AddTodoInput"), "{ts}");
    assert!(ts.contains("label: string | null;"), "{ts}");
    assert!(
        ts.contains("import type { ActionCall, ActionOptions } from './client.ts'"),
        "{ts}"
    );
    assert!(!ts.contains("ActionClientContract"), "{ts}");
    assert!(
        ts.contains("addTodo: (args:AddTodoInput, options?:AddTodoOptions):Promise<ActionCall<AddTodoOutput>> => port.invokeAction('AddTodo',1,encodeAddTodoInput(args),decodeAddTodoOutput,options)"),
        "{ts}"
    );
    assert!(
        ts.contains("addTodo: (args:AddTodoInput, options?:AddTodoOptions):Promise<AddTodoOutput> => port.invokeDirectAction('AddTodo',1,encodeAddTodoInput(args),decodeAddTodoOutput,options)"),
        "{ts}"
    );
    assert!(!ts.contains("class GeneratedClient {"), "{ts}");
}

#[test]
fn action_backend_emits_versioned_handler_identity_contracts_with_factory() {
    let v = compile("model Todo { id String title String @@id(id) } action AddTodo(todo Todo.create) { relatedTodo Todo? }").unwrap();
    let mut retained = v.clone();
    let mut old = retained["actions"][0].clone();
    old["outputs"].as_array_mut().unwrap().pop();
    old["input"] = serde_json::json!({"models": [v["schema"]["models"][0].clone()], "enums": []});
    old["outputEnums"] = serde_json::json!([]);
    let mut current = v["actions"][0].clone();
    current["version"] = serde_json::json!(2);
    retained["actions"] = serde_json::json!([old, current]);
    let ts = axton_compiler::backend_typescript(&retained, "@axton/server");
    assert!(
        ts.contains("export type AddTodoV1HandlerOutput = void;"),
        "{ts}"
    );
    assert!(ts.contains("export interface AddTodoHandlerOutput"), "{ts}");
    assert!(ts.contains("relatedTodo: TodoIdentity | null;"), "{ts}");
    assert!(
        ts.contains(
            "v1(call: ActionHandlerCall<Tx, AddTodoV1Input>): Promise<AddTodoV1HandlerOutput>"
        ),
        "{ts}"
    );
    assert!(
        ts.contains("v2(call: ActionHandlerCall<Tx, AddTodoInput>): Promise<AddTodoHandlerOutput>"),
        "{ts}"
    );
    assert!(ts.contains("export function createBackend<Tx>"), "{ts}");
    assert!(ts.contains("createRuntimeBackend"), "{ts}");
    assert!(ts.contains("BackendOptions"), "{ts}");
}

#[test]
fn mixed_action_and_legacy_backend_keeps_handler_context_in_scope() {
    let v = compile("model Todo { id String @@id(id) } mutation Legacy { todo Todo.delete } action New(todo Todo.delete)").unwrap();
    let ts = axton_compiler::backend_typescript(&v, "@axton/server");
    assert!(
        ts.contains("legacy: { v1(call: HandlerCall<Tx, LegacyInput>)"),
        "{ts}"
    );
    assert!(
        ts.contains("new: { v1(call: ActionHandlerCall<Tx, NewInput>)"),
        "{ts}"
    );
    assert!(ts.contains("export function createBackend<Tx>"), "{ts}");
}

#[test]
fn action_dart_emits_concrete_client_and_versioned_handler_contracts() {
    let v = compile("model Todo { id String title String @@id(id) } action Search(query String?) { relatedTodo Todo? } action Ping()").unwrap();
    let dart = axton_compiler::dart(&v);
    for expected in [
        "show RuntimeConnection, SyncServer, ActionCall, ActionOutcome, ActionSuccess, ActionFailure, ActionStatus, ActionError",
        "required String? query",
        "class TodoIdentity",
        "required this.relatedTodo",
        "TodoIdentity? relatedTodo",
        "Todo? relatedTodo",
        "late final Actions actions = Actions(client);",
        "late final DirectCalls call = DirectCalls(client);",
        "Future<ActionCall<SearchOutput>> search(",
        "Future<SearchOutput> search(",
        "typedef PingOutput = void;",
    ] {
        assert!(dart.contains(expected), "missing {expected}: {dart}");
    }
    assert!(!dart.contains("ActionClientContract"));
}

#[test]
fn model_only_dart_keeps_local_crud_without_action_symbols() {
    let v = compile("model Note { id String label String @@id(id) }").unwrap();
    let dart = axton_compiler::dart(&v);
    assert!(
        dart.contains("class NoteLiveModel extends NoteTxModel"),
        "{dart}"
    );
    assert!(!dart.contains("late final Actions actions"), "{dart}");
}

#[test]
fn action_dart_binds_shared_runtime_and_retained_codecs() {
    let v = compile("enum Mood { calm loud } model Note { id String at DateTime mood Mood @@id(id) } action Save(note Note.create, changed Note.update<at>?, stamps DateTime[], when DateTime?) { saved Note? at DateTime moods Mood[] }").unwrap();
    let dart = axton_compiler::dart(&v);
    for expected in [
        "show RuntimeConnection, SyncServer, ActionCall, ActionOutcome, ActionSuccess, ActionFailure, ActionStatus, ActionError",
        "late final Actions actions = Actions(client)",
        "client.invokeAction<SaveOutput>('Save', 1",
        "client.invokeDirectAction<SaveOutput>('Save', 1",
        "Duration directTimeout = const Duration(seconds: 30)",
        "directTimeout:directTimeout",
        "DateTime.parse(",
        ".toUtc().toIso8601String()",
        "if (changed != null)",
        "class NoteLiveModel extends NoteTxModel",
    ] {
        assert!(dart.contains(expected), "missing {expected}: {dart}");
    }
    assert!(
        !dart.contains("abstract interface class ActionCall<T>"),
        "{dart}"
    );
}

#[test]
fn action_dart_retains_output_read_version_and_enum_snapshot() {
    let mut v = compile("enum Status { open closed } model Todo { id String title String @@id(id) @@version(2) } action Add() { related Todo? state Status }").unwrap();
    let mut old = v["actions"][0].clone();
    old["version"] = serde_json::json!(1);
    old["outputs"][0]["modelReadVersion"] = serde_json::json!(1);
    old["outputEnums"] = serde_json::json!([{"name":"Status","values":["open"]}]);
    old["input"] = serde_json::json!({"models":[],"enums":[]});
    let mut current = v["actions"][0].clone();
    current["version"] = serde_json::json!(2);
    v["actions"] = serde_json::json!([old, current]);
    v["backendModels"] = serde_json::json!([{"name":"Todo","version":1,"identity":["id"],"fields":[{"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},{"name":"title","type":{"kind":"scalar","name":"string"},"nullable":false}]}]);
    let dart = axton_compiler::dart(&v);
    assert!(dart.contains("class TodoV1Identity"), "{dart}");
    assert!(dart.contains("TodoV1Identity? related"), "{dart}");
    assert!(dart.contains("enum AddV1OutputStatus { open }"), "{dart}");
    assert!(dart.contains("AddV1OutputStatus state"), "{dart}");
}

#[test]
fn action_generated_identifiers_reject_current_collisions_with_positions() {
    let cases = [
        (
            "model FetchInput { id String @@id(id) } action Fetch()",
            "FetchInput",
        ),
        (
            "model FetchOutput { id String @@id(id) } action Fetch()",
            "FetchOutput",
        ),
        (
            "model FetchHandlerOutput { id String @@id(id) } action Fetch()",
            "FetchHandlerOutput",
        ),
        (
            "model Todo { id String @@id(id) } mutation Fetch { todo Todo.create } action Fetch()",
            "FetchInput",
        ),
        (
            "enum ActionOutcome { open } model Todo { id String @@id(id) } action Fetch()",
            "ActionOutcome",
        ),
        (
            "model ActionSuccess { id String @@id(id) } action Fetch()",
            "ActionSuccess",
        ),
        (
            "model ActionFailure { id String @@id(id) } action Fetch()",
            "ActionFailure",
        ),
        (
            "model ActionBackendContract { id String @@id(id) } action Fetch()",
            "ActionBackendContract",
        ),
        (
            "model Actions { id String @@id(id) } action Fetch()",
            "Actions",
        ),
        (
            "enum DirectCalls { open } model Todo { id String @@id(id) } action Fetch()",
            "DirectCalls",
        ),
        (
            "model ActionPort { id String @@id(id) } action Fetch()",
            "ActionPort",
        ),
        (
            "enum ActionContext { open } model Todo { id String @@id(id) } action Fetch()",
            "ActionContext",
        ),
        (
            "model ActionRejected { id String @@id(id) } action Fetch()",
            "ActionRejected",
        ),
        (
            "model Todo { id String @@id(id) } model ActionTodoModel { id String @@id(id) } action Fetch()",
            "ActionTodoModel",
        ),
        (
            "model Todo { id String title String @@id(id) } model FetchTodoUpdate { id String @@id(id) } action Fetch(todo Todo.update<title>)",
            "FetchTodoUpdate",
        ),
    ];
    for (source, name) in cases {
        let error = compile(source).unwrap_err();
        assert!(
            error.contains(name)
                && (error.contains("Action") || error.contains("generated client"))
                && error.starts_with("1:"),
            "{source}: {error}"
        );
    }
}

#[test]
fn action_generated_identifiers_reject_other_actions_without_overbanning() {
    assert!(
        compile("model Todo { id String @@id(id) } action Fetch() action FetchInput()").is_ok()
    );
    assert!(compile("model FetchV1Input { id String @@id(id) } action Fetch()").is_ok());
}

#[test]
fn backend_enum_list_handler_outputs_preserve_latest_and_retained_union_cardinality() {
    let old = compile("enum Status { open closed } model Todo { id String @@id(id) } action Fetch() { states Status[] }").unwrap();
    let history = axton_compiler::reconcile_action_history(&old, None).unwrap();
    let mut latest = compile("enum Status { open closed archived } model Todo { id String @@id(id) } @version(2) action Fetch() { states Status[] }").unwrap();
    let history = axton_compiler::reconcile_action_history(&latest, Some(&history)).unwrap();
    latest["actions"] = serde_json::Value::Array(
        history["actions"]["Fetch"]
            .as_object()
            .unwrap()
            .values()
            .cloned()
            .collect(),
    );
    let emitted = axton_compiler::backend_typescript(&latest, "@axton/server");
    let interface = |name: &str| {
        let marker = format!("export interface {name} {{");
        emitted
            .split_once(&marker)
            .unwrap()
            .1
            .split_once("}\n")
            .unwrap()
            .0
    };
    assert!(
        interface("FetchV1HandlerOutput").contains("states: (\"open\" | \"closed\")[];"),
        "{emitted}"
    );
    assert!(
        interface("FetchHandlerOutput")
            .contains("states: (\"open\" | \"closed\" | \"archived\")[];"),
        "{emitted}"
    );
}

#[test]
fn action_only_and_model_only_clients_have_no_legacy_mutate_facade() {
    for schema in [
        "model Todo { id String @@id(id) } action AddTodo(todo Todo.create)",
        "model Todo { id String @@id(id) }",
    ] {
        let v = compile(schema).unwrap();
        let ts = axton_compiler::typescript(&v);
        let dart = axton_compiler::dart(&v);
        assert!(!ts.contains("export class Mutate"), "{ts}");
        assert!(!ts.contains("export interface MutatePort"), "{ts}");
        assert!(!dart.contains("class Mutate"), "{dart}");
        assert!(!dart.contains("late final Mutate mutate"), "{dart}");
        let client = axton_compiler::client_typescript(&v, "@axton/client");
        assert!(!client.contains("readonly mutate"), "{client}");
        assert!(!client.contains("new Mutate(client)"), "{client}");
        let backend = axton_compiler::backend_typescript(&v, "@axton/server");
        assert!(!backend.contains("MutationRejected"), "{backend}");
    }
}

/// The Scope facade is the public spelling the generated client carries: a thin
/// delegate to the runtime, with the handle types named through the generated
/// module and no get-only accessor
/// ([#150](https://github.com/zanminwang/axton/issues/150)).
#[test]
fn generated_clients_expose_the_scope_facade() {
    let schema = compile("model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> }").unwrap();
    let ts = axton_compiler::client_typescript(&schema, "@example/custom-runtime");
    for line in [
        "type Subscription, type SubscriptionStatus",
        " subscribe(scope: string): Promise<Subscription> { return this.#client.subscribeScope(scope); }",
        " readonly scopes: Scopes;",
        "this.scopes = new Scopes(client);",
        " subscribe(channel: string): Promise<Subscription> { return this.#client.subscribe(channel); }",
    ] {
        assert!(ts.contains(line), "{line} missing from {ts}");
    }
    assert!(
        !ts.contains("get(scope"),
        "a get-only accessor is deliberately omitted: {ts}"
    );
    let dart = axton_compiler::dart(&schema);
    for line in [
        "Subscription, SubscriptionStatus, SubscriptionInitialization, SubscriptionConnection",
        "class Scopes { final Client client; Scopes(this.client);",
        " Future<Subscription> subscribe(String scope) => client.subscribeScope(scope);",
        " late final Scopes scopes = Scopes(client);",
        " Future<Subscription> subscribe(String channel) => client.subscribe(channel);",
    ] {
        assert!(dart.contains(line), "{line} missing from {dart}");
    }
    assert!(
        !dart.contains("get(String scope"),
        "a get-only accessor is deliberately omitted: {dart}"
    );
}

#[test]
fn action_store_options_name_only_explicit_model_outputs_in_both_languages() {
    let v = compile("model Todo { id String title String @@id(id) } action AddTodo(todo Todo.create, gone Todo.delete[]) { related Todo? matches Todo[] count Int } action Ping() action Open(store String) { main Todo }").unwrap();
    let ts = axton_compiler::typescript(&v);
    // Input-bound, Delete-confirmation and scalar outputs are not keys.
    assert!(
        ts.contains("export type AddTodoOptions = ActionOptions<'related'|'matches'>;"),
        "{ts}"
    );
    assert!(
        ts.contains("export type PingOptions = { store?: boolean };"),
        "{ts}"
    );
    assert!(
        ts.contains("export type OpenOptions = ActionOptions<'main'>;"),
        "{ts}"
    );
    let client = axton_compiler::client_typescript(&v, "@axton/client");
    assert!(client.contains("type ActionOptions"), "{client}");
    let dart = axton_compiler::dart(&v);
    assert!(
        dart.contains("const AddTodoStore.outputs({this.related, this.matches}) : _mode = 2;"),
        "{dart}"
    );
    assert!(
        dart.contains("const PingStore.none() : _mode = 1;"),
        "{dart}"
    );
    assert!(!dart.contains("PingStore.outputs"), "{dart}");
    assert!(
        dart.contains("Future<ActionCall<PingOutput>> ping({PingStore? store})"),
        "{dart}"
    );
    // A business input named store keeps its name; the selector moves aside.
    assert!(
        dart.contains("open({required String store, OpenStore? outputStore})"),
        "{dart}"
    );
    assert!(dart.contains("store: outputStore);"), "{dart}");
    // The option is a call-time choice: no descriptor or history policy.
    let descriptor = serde_json::to_string(&v).unwrap();
    assert!(!descriptor.contains("ephemeral"), "{descriptor}");
    for output in v["actions"][0]["outputs"].as_array().unwrap() {
        assert!(output.get("store").is_none(), "{output}");
    }
}

#[test]
fn action_store_selector_names_join_generated_identifier_checks() {
    let error = compile("model PingStore { id String @@id(id) } action Ping()").unwrap_err();
    assert!(error.contains("PingStore"), "{error}");
    let error = compile("model PingOptions { id String @@id(id) } action Ping()").unwrap_err();
    assert!(error.contains("PingOptions"), "{error}");
}

#[test]
fn store_eligible_outputs_cannot_reuse_dart_selector_member_names() {
    for member in [
        "toWire",
        "toString",
        "hashCode",
        "runtimeType",
        "noSuchMethod",
    ] {
        let error = compile(&format!(
            "model Todo {{ id String @@id(id) }} action Find() {{ {member} Todo? }}"
        ))
        .unwrap_err();
        assert!(
            error.contains(&format!("output {member} is reserved")),
            "{error}"
        );
        assert!(error.contains("FindStore"), "{error}");
    }
    // A scalar output is not a selector field and keeps the name.
    compile("action Count() { toWire Int }").unwrap();
    // Other store-eligible names still compile.
    compile("model Todo { id String @@id(id) } action Find() { all Todo? none Todo[] }").unwrap();
}

#[test]
fn generated_dart_reexports_action_store() {
    let v = compile("action Ping()").unwrap();
    let dart = axton_compiler::dart(&v);
    assert!(dart.contains("ActionError, ActionStore,"), "{dart}");
}
