use ahead_compiler::compile;
use ahead_compiler::validate::{
    ActionInput, ActionOutputSource, ActionOutputType, Cardinality, FieldType, Scalar,
};
use ahead_compiler::{parse, validate};

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
    let ts = ahead_compiler::typescript(&v);
    assert!(ts.contains("export interface EntryIdentity"));
    assert!(ts.contains("new Date("));
    assert!(ts.contains("EditEntry"));
    let dart = ahead_compiler::dart(&v);
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
    // `Parent.update<>` is valid: an empty patch is a no-op update ([#49](https://github.com/zanminwang/ahead/issues/49)).
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
    let ts = ahead_compiler::backend_typescript(&v, "@ahead/server");
    assert!(ts.contains("from \"@ahead/server\""));
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
    assert!(ts.contains("export function Book(identity: BookIdentity): RecordRef { return { model: \"Book\", identity }; }"));
    assert!(ts.contains("export interface AddBookInput {\n book: Book;\n}"));
    assert!(ts.contains("export function createBackend<Tx>("));
    assert!(!ahead_compiler::typescript(&v).contains("backendConfig"));
}
#[test]
fn backend_emitter_groups_handler_versions_under_the_mutation_name() {
    let v = compile("model A { id String title String @@id(id) } mutation Edit { a A.update<title> @@version(2) }").unwrap();
    let mut with_history = v.clone();
    let mut old = v["mutations"][0].clone();
    old["version"] = serde_json::json!(1);
    with_history["backendMutations"] = serde_json::json!([old, v["mutations"][0].clone()]);
    let ts = ahead_compiler::backend_typescript(&with_history, "@ahead/server");
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
    let ts = ahead_compiler::backend_typescript(&v, "@ahead/server");
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
    let ts = ahead_compiler::backend_typescript(&later, "@ahead/server");
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
    let ts = ahead_compiler::client_typescript("@example/custom-runtime");
    assert!(ts.contains("type ServerOptions"));
    assert!(ts.contains("server?: ServerOptions"));
    assert!(ts.contains("client.connect(options.server"));
    assert!(!ts.contains("LiveTransport"));
    assert!(!ts.contains("transport?:"));
    assert!(
        ts.find("connection options were removed").unwrap() < ts.find("await Client.open").unwrap()
    );
    let schema = compile("model Entry { id String title String @@id(id) } mutation Edit { entry Entry.update<title> }").unwrap();
    let dart = ahead_compiler::dart(&schema);
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
    let ts = ahead_compiler::client_typescript("@example/custom-runtime");
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
    let model = ahead_compiler::typescript(&schema);
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
    let dart = ahead_compiler::dart(&schema);
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
    let ts = ahead_compiler::typescript(&schema);
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
    let ts_client = ahead_compiler::client_typescript("@example/custom-runtime");
    assert!(
        ts_client.contains("this.mutate = new Mutate(client)"),
        "{ts_client}"
    );

    let dart = ahead_compiler::dart(&schema);
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
        "Transaction",
    ] {
        let e = compile(&format!("model {name} {{ id UUID @@id(id) }}")).unwrap_err();
        assert!(e.contains("generated client"), "{name}: {e}");
        let e = compile(&format!(
            "enum {name} {{ a b }}\nmodel Other {{ id UUID @@id(id) }}"
        ))
        .unwrap_err();
        assert!(e.contains("generated client"), "enum {name}: {e}");
    }
    for name in ["Status", "SyncStates", "Rejections", "Order"] {
        assert!(
            compile(&format!("model {name} {{ id UUID @@id(id) }}")).is_ok(),
            "{name} should stay valid"
        );
    }
}

#[test]
fn rejects_reserved_model_names_at_the_declaration() {
    for name in ["sqlite_entry", "SQLite_Entry", "ahead_entry", "Ahead_entry"] {
        let e = compile(&format!(
            "model Other {{ id UUID @@id(id) }}\n\nmodel {name} {{ id UUID @@id(id) }}\n\n\n"
        ))
        .unwrap_err();
        assert!(e.contains("reserved"), "{name}: {e}");
        assert_eq!(line_of(&e), 3, "{name}: {e}");
    }
    for name in ["Sqlite", "sqlitex", "my_sqlite_table", "aheadEntry"] {
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
    let ts = ahead_compiler::typescript(&v);
    assert!(
        ts.contains(r#""name":"Task","relations":[],"unique":[],"version":2}"#),
        "{ts}"
    );
    assert!(
        ts.contains(r#""name":"Note","relations":[],"unique":[],"version":1}"#),
        "{ts}"
    );
    let dart = ahead_compiler::dart(&v);
    assert!(
        dart.contains(r#""name":"Task","relations":[],"unique":[],"version":2}"#),
        "{dart}"
    );
    let mut with_history = v.clone();
    let old = serde_json::json!({"name":"Task","version":1,"identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"uuid"}}],"enums":[]});
    with_history["backendModels"] = serde_json::json!([old]);
    let backend = ahead_compiler::backend_typescript(&with_history, "@ahead/server");
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
    let ts = ahead_compiler::backend_typescript(&with_history, "@ahead/server");
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
        ts.contains(" task: { v1(call: LoaderCall<Tx, TaskIdentity>): Promise<readonly (TaskV1 | null)[]>; v2(call: LoaderCall<Tx, TaskIdentity>): Promise<readonly (Task | null)[]> };\n"),
        "{ts}"
    );
    assert!(
        ts.contains(" note: { v1(call: LoaderCall<Tx, NoteIdentity>): Promise<readonly (Note | null)[]> } | ((call: LoaderCall<Tx, NoteIdentity>) => Promise<readonly (Note | null)[]>);\n"),
        "{ts}"
    );
    // Without a history the schema's own version is the only retained one; a
    // single non-v1 version has no shorthand.
    let ts = ahead_compiler::backend_typescript(&v, "@ahead/server");
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
    let ts = ahead_compiler::typescript(&v);
    assert!(ts.contains("export interface Task {\n id: string;\n /** @deprecated renamed to title */\n name: string;\n title: string;\n /** @deprecated */\n legacy: number | null;\n"), "{ts}");
    assert!(
        ts.contains(
            "export interface TaskPatch {\n /** @deprecated renamed to title */\n name?: string;\n"
        ),
        "{ts}"
    );
    assert!(ts.contains("/** @deprecated \"archived\": use closed */\nexport type Status = \"active\" | \"archived\" | \"closed\";"), "{ts}");
    assert!(ts.contains("export interface EditArgs {\n task: { identity:TaskIdentity; values:Pick<TaskPatch, \"title\"> };\n /** @deprecated use task */\n old?: { identity:TaskIdentity; values:Pick<TaskPatch, \"name\"> };\n}"), "{ts}");
    let backend = ahead_compiler::backend_typescript(&v, "@ahead/server");
    assert!(backend.contains("export interface EditInput {\n task: { identity: TaskIdentity; patch: Pick<TaskPatch, \"title\"> };\n /** @deprecated use task */\n old: { identity: TaskIdentity; patch: Pick<TaskPatch, \"name\"> } | null;\n}"), "{backend}");
    let dart = ahead_compiler::dart(&v);
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
    let descriptor = ahead_compiler::compile(source).unwrap();
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
    let descriptors = ahead_compiler::compile(source).unwrap();
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
    let history = ahead_compiler::reconcile_action_history(&descriptors, None).unwrap();
    let snapshot = &history["actions"]["Add"]["1"];
    assert_eq!(snapshot["requirements"][0]["name"], "Uploaded");
    assert_eq!(snapshot["prerequisites"][0]["name"], "Uploaded");
    assert_eq!(snapshot["sequence"], action["sequence"]);
}
