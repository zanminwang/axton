//! Typed Mutation input on client and transaction.
//! Schemas without a current Mutation retain their local-only transaction.
use crate::current_operations::CurrentOperations;
use crate::emit::{arr, dart_action_decoder, lower, s};
use axton_core::CallKind;
use std::fmt::Write;

/// The facade `client.transaction` passes to its callback: the application
/// transaction beside a current Mutation, otherwise the local-only one.
pub(crate) fn transaction_type(current: &CurrentOperations<'_>) -> &'static str {
    if current.has_mutations() {
        "ApplicationTransaction"
    } else {
        "GeneratedTransaction"
    }
}

/// The raw submission port, `makeTransactionMutations` and
/// `ApplicationTransaction`. Each method encodes its typed input object; a
/// caller that reads first does so in the same transaction.
pub(crate) fn ts_generated(current: &CurrentOperations<'_>, o: &mut String) {
    let mutations: Vec<_> = current.of_kind(CallKind::Mutation).collect();
    if mutations.is_empty() {
        return;
    }
    o.push_str("export interface SubmitMutationPort { submitMutation<T>(name:string,version:number,input:object,decode:(value:unknown)=>T):Promise<Call<T>>; }\nexport interface UnsentResolutionPort { readonly rejections:{ dismiss(id:number):Promise<void> }; readonly failures:{ retry(taskKeys:string[]):Promise<void>; drop(ordinal:number):Promise<void> }; }\n");
    o.push_str("export function makeTransactionMutations(port:SubmitMutationPort) { return {\n");
    for (name, version, _) in &mutations {
        writeln!(o, " {}: (input:{name}Input):Promise<Call<{name}Output>> => port.submitMutation('{name}',{version},encode{name}Input(input),decode{name}Output),", lower(name)).unwrap();
    }
    o.push_str("}; }\n");
    o.push_str("export class ApplicationTransaction extends GeneratedTransaction { readonly mutations:ReturnType<typeof makeTransactionMutations>; readonly rejections:UnsentResolutionPort['rejections']; readonly failures:UnsentResolutionPort['failures']; constructor(transaction:ReadPort & SubmitMutationPort & UnsentResolutionPort) { super(transaction); this.mutations=makeTransactionMutations(transaction); this.rejections=transaction.rejections; this.failures=transaction.failures; } }\n");
}

/// The Dart twin of [`ts_generated`]. `SubmitMutationPort` is the runtime's
/// (`package:axton`), which the raw `Transaction` implements.
pub(crate) fn dart_generated(current: &CurrentOperations<'_>, o: &mut String) {
    let mutations: Vec<_> = current.of_kind(CallKind::Mutation).collect();
    if mutations.is_empty() {
        return;
    }
    o.push_str("class TransactionMutations {\n final SubmitMutationPort port; TransactionMutations(this.port);\n");
    for (name, _, _) in &mutations {
        writeln!(
            o,
            " late final {name}Mutation {} = {name}Mutation(port);",
            lower(name)
        )
        .unwrap();
    }
    o.push_str("}\n");
    for (name, version, action) in &mutations {
        let arguments = arr(action, "inputs")
            .iter()
            .map(|input| {
                let key = s(input, "name");
                format!("'{key}': _dartActionEncode(input.{key})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(o, "class {name}Mutation {{\n final SubmitMutationPort port; {name}Mutation(this.port);\n Map<String,dynamic> _encode({name}Input input) => {{{arguments}}};\n Future<Call<{name}Output>> call({name}Input input) => port.submitMutation<{name}Output>('{name}', {version}, _encode(input), {});\n}}", dart_action_decoder(action,name)).unwrap();
    }
    o.push_str("class ApplicationTransaction extends GeneratedTransaction { late final TransactionMutations mutations = TransactionMutations(transaction); late final rejections = transaction.rejections; late final failures = transaction.failures; ApplicationTransaction(super.transaction); }\n");
}
