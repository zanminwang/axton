//! Named Mutation scopes share one typed submission contract on client and tx.
use axton_compiler::compile;
const SCHEMA: &str = "model Draft { id String text String @@id(id) } model Entry { id String text String @@id(id) } mutation Publish(entry Entry.create, call String) { entry Entry } query Find(id String) { entry Entry? }";
#[test]
fn typescript_application_transactions_queue_typed_mutations() {
    let v = compile(SCHEMA).unwrap();
    let ts = axton_compiler::typescript(&v);
    let client = axton_compiler::client_typescript(&v, "@axtonjs/client");
    for expected in [
        "export interface SubmitMutationPort",
        "publish: (input:PublishInput):Promise<Call<PublishOutput>> => port.submitMutation('Publish',1,encodePublishInput(input),decodePublishOutput)",
        "submitMutation<T>(name:string,version:number,input:object,decode:(value:unknown)=>T)",
        "return makeTransactionMutations(port);",
        "readonly mutations:ReturnType<typeof makeTransactionMutations>",
    ] {
        assert!(ts.contains(expected), "{expected}");
    }
    assert!(client.contains("new ApplicationTransaction(tx)"));
    // The Mutation input callback is retired: a caller reads in the
    // transaction, then submits.
    for retired in [
        "CompanionOptions",
        "CompanionContext",
        "typeof input === 'function'",
    ] {
        assert!(!ts.contains(retired), "{retired}");
    }
    assert!(!ts.contains(" call: {"));
}
#[test]
fn dart_application_transactions_queue_typed_mutations() {
    let v = compile(SCHEMA).unwrap();
    let dart = axton_compiler::dart(&v);
    for expected in [
        "class TransactionMutations",
        "class PublishMutation",
        "call(PublishInput input)",
        "class ApplicationTransaction",
    ] {
        assert!(dart.contains(expected), "{expected}");
    }
    for retired in [
        "class DirectMutations",
        "CompanionContext",
        "withTransaction",
        "input:",
    ] {
        assert!(!dart.contains(retired), "{retired}");
    }
}
// Model facades only read: every row change is a Mutation. There is no
// write port, no transaction Model facade with writes, and no create,
// update or delete on a client or transaction Model.
#[test]
fn model_facades_are_read_only_in_both_languages() {
    let v = compile(SCHEMA).unwrap();
    let ts = axton_compiler::typescript(&v);
    for expected in [
        "export interface LivePort extends ReadPort {",
        "export class EntryLiveModel extends EntryModel<LivePort> {",
        "export interface TxModels { draft:DraftModel; entry:EntryModel; }",
        "export function txModels(port:ReadPort):TxModels { return { draft:new DraftModel(port), entry:new EntryModel(port) }; }",
        "export class GeneratedTransaction { readonly transaction:ReadPort; readonly models:TxModels; constructor(transaction:ReadPort) {",
    ] {
        assert!(ts.contains(expected), "{expected}");
    }
    for retired in [
        "WritePort",
        "TxModel<",
        "direct(",
        " create(value:",
        " update(identity:",
        " delete(identity:",
        "CreateIdentity",
    ] {
        assert!(!ts.contains(retired), "{retired}");
    }
    let dart = axton_compiler::dart(&v);
    for expected in [
        "class EntryLiveModel extends EntryModel {",
        "class TxModels { final ReadPort port; TxModels(this.port);",
        " late final EntryModel entry = EntryModel(port);",
    ] {
        assert!(dart.contains(expected), "{expected}");
    }
    for retired in [
        "WritePort",
        "TxModel extends",
        "direct(",
        "Future<void> create(",
        "Future<void> update(",
        "Future<void> delete(",
        "class EntryPatch",
    ] {
        assert!(!dart.contains(retired), "{retired}");
    }
}
#[test]
fn dart_controls_never_collide_with_business_inputs() {
    let v = compile("mutation Mark(local Boolean, store String, call String)").unwrap();
    let dart = axton_compiler::dart(&v);
    for field in [
        "final bool local;",
        "final String store;",
        "final String call;",
    ] {
        assert!(dart.contains(field));
    }
    assert!(dart.contains("call(MarkInput input)"));
    assert!(!dart.contains("MarkStore"));
}
#[test]
fn only_current_mutations_join_the_transaction_facade() {
    let mut v = compile("mutation Ping() query Find() { count Int }").unwrap();
    let mut old = v["actions"][0].clone();
    old["kind"] = "mutation".into();
    old["version"] = 0.into();
    old["input"] = serde_json::json!({"enums":[],"models":[]});
    old["outputEnums"] = serde_json::json!([]);
    v["actions"].as_array_mut().unwrap().push(old);
    let ts = axton_compiler::typescript(&v);
    let dart = axton_compiler::dart(&v);
    assert!(ts.contains("ping: (input:PingInput"));
    assert!(!ts.contains("find: (input:"));
    assert!(dart.contains("late final PingMutation ping"));
    assert!(!dart.contains("FindMutation"));
}
#[test]
fn schemas_without_mutations_keep_the_local_transaction() {
    for source in [
        "model Entry { id String @@id(id) }",
        "query Ping() { value String }",
        "model Entry { id String @@id(id) } query Find() { entry Entry? }",
    ] {
        let v = compile(source).unwrap();
        for text in [
            axton_compiler::typescript(&v),
            axton_compiler::dart(&v),
            axton_compiler::client_typescript(&v, "runtime"),
        ] {
            assert!(!text.contains("ApplicationTransaction"));
            assert!(!text.contains("CompanionContext"));
            assert!(!text.contains("TransactionMutations"));
        }
    }
}
#[test]
fn transaction_mutation_helper_names_are_reserved_beside_mutations() {
    for name in [
        "ApplicationTransaction",
        "SubmitMutationPort",
        "TransactionMutations",
        "UnsentResolutionPort",
    ] {
        let source = format!("model {name} {{ id String @@id(id) }} mutation Ping()");
        assert!(compile(&source).unwrap_err().contains("collides"));
    }
    for name in [
        "CompanionContext",
        "CompanionOptions",
        "SubmitMutationOptions",
    ] {
        assert!(
            compile(&format!(
                "model {name} {{ id String @@id(id) }} mutation Ping()"
            ))
            .is_ok()
        );
    }
}
