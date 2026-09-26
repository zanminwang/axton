# Model creation defaults

Issue: [#27](https://github.com/zanminwang/axton/issues/27). Based on main `ceced50`, after Query/Mutation #157. The user approved shipping literal and enum defaults, `uuid()` and `now()` together.

## Public contract

```text
enum Status { open closed }
model Todo {
  id String @default(uuid())
  title String @default("")
  done Boolean @default(false)
  priority Int @default(0)
  status Status @default(open)
  createdAt DateTime @default(now())
  @@id(id)
}
mutation AddTodo(todo Todo.create) {}
```

`@default` is a stored Model field attribute, not an operation input/output attribute. Support String, Boolean/Bool, Int, Float and enum literals; quoted UUID and DateTime literals use their existing normalization rules. `uuid()` is allowed on String and UUID, produces canonical lowercase UUID v4; `now()` is allowed only on DateTime, produces client wall-clock UTC time at millisecond precision. Both functions take zero arguments. Reject unknown functions, wrong literal/function types, duplicate defaults, relation/list defaults and `@default(null)` in this milestone. Nullable fields can have a non-null default.

Only an omitted field of a fresh create receives a default. An explicit value wins. An explicit null remains null if nullable and otherwise errors; it never requests a default. Update omission still means no change. Delete, identity lookup, Loader normalization, incoming authority and replay do not evaluate creation defaults. A nullable field omitted without a default retains existing normalization to null.

Apply to local Model create, create inside a local transaction, and Model.create operands of durable and direct Mutations, including optional and list operands. Generated identity values must exist before record-key normalization and relation-binding validation. Each omitted generated field is evaluated once per fresh create; a second independent create generates new values. Multiple now() fields need not share exactly the same instant. Client clocks do not provide trusted server timestamps.

Generated TypeScript create inputs make only defaulted fields optional relative to the existing full record contract; encoders preserve missing fields. Dart must distinguish omitted defaulted nullable fields from explicit null, using the existing Present pattern where needed. Keep full Model/read/output types complete, and generated backend handler create arguments complete: handlers receive expanded values. Preserve existing local create completion/return contract; this issue does not add a new returned row or identity API. Callers requiring an identity before submission can supply it explicitly; persisted generated values are visible through Model reads/watch and declared Mutation outputs.

## Shared execution boundary

Put evaluation in a focused client Rust module (suggested `crates/client/src/defaults.rs`), not in generic shared state normalization or generated JS/Dart code. Shared core code validates metadata; generic normalizers stay deterministic and strict on missing required data.

For local create, fill omitted identity and state fields before `client::mutate::normalize` validates the key/state. Apply the same rule to fresh low-level enqueue operations where appropriate, while never regenerating concrete canonical Action intents or replayed rows.

For durable/direct Mutations, prefill create operands before `normalize_action_args` in `submit_action_with_options` and `prepare_action_with_options`. Use current Model default policy restricted to fields of the retained operation's input Model contract; never inject fields absent from that contract. Normalize and validate the expanded arguments against the retained input schema. Those exact expanded arguments drive optimistic operations, persisted intent, direct request and backend execution. Retrying transport, reopening, freezing, settlement and replay consume concrete values and do not call the generator. Server normalization does not synthesize omitted required defaults for malformed client requests.

Use a separate optional `FieldDescriptor.createDefault` tagged contract: `{kind:"literal", value:...}`, `{kind:"uuid"}` or `{kind:"now"}`. The existing internal literal `default` metadata serves older reconciliation/read-projection paths and is not the target of source `@default`. This implements the user clarification that *all* source defaults trigger only on create, including constants. Validate createDefault metadata in core and compiler; reject unknown tags, malformed values and wrong field types, and preserve omitted metadata compatibility.

## Versions, persistence and reconciliation

Changing only an existing field's creation default does not change its backend wire/read shape and must not require a Model or Mutation version bump, rebuild local data or mutate queued/frozen values. Keep createDefault on current client Model descriptors. Project it out of retained Model read snapshots, resultModels and operation-input snapshots: those describe complete backend values, not client creation policy. Preserve the pre-existing internal default metadata and its semantics. History compatibility compares existing field shape independently of creation metadata while retaining the identity, enum and added-required-field rules. Normalize away createDefault when comparing hand-authored snapshots if necessary; prior structural versions remain retained. Old concrete intents remain valid after changing or removing a default.

Adding a required field remains a structural backend/read-contract change and follows existing version rules even when it has a default. Do not use this feature to weaken retained Loader contracts. Source `@default` never grants migration/backfill permission, even for literals. Keep pre-existing internal literal-default backfill paths compatible, but do not populate them from createDefault. A new required column still needs the existing migration/rebuild handling and explicit application-supplied historical values where required. Nullable additions follow existing null-fill rules. Never invent a default, UUID or current creation time for historical rows. Changing a default does not rewrite existing rows.

Do not use defaults to mask malformed Loader records. Preserve existing explicit read-contract projection behavior; createDefault (literal or dynamic) must never run during old-to-current authority projection. Enum/string literals must be normalized before storage and validated against their field contract.

## Code generation and escaping

Separate client create inputs/encoders from full record and backend handler inputs. Ensure Model-only schemas expose usable create inputs even without remote operations. Handle retained versions, list/optional create operands, generated identity fields and explicit nullable values consistently in TS and Dart. Existing full Model instances should remain usable for explicit-value Dart create calls where practical through a shared create-input interface, without weakening full-record types.

Treat default strings as data in generated sources. JSON literals containing quotes, backslashes, newlines, dollar signs and Dart triple quotes must remain valid and round-trip identically. Replace unsafe raw triple-quoted schema embedding with a safely escaped representation rather than rejecting valid strings.

## Verification and boundaries

Tests cover source and descriptor rejection, every supported default, full backend handler types, local and remote create routes, explicit overrides/nulls, update/Loader strictness, optional/list creates, and generated strings. Real SQLite tests cover transaction rollback, queued/frozen defaults surviving reopen/replay and default-only schema evolution without rewriting data. Runtime integration verifies both direct and durable handlers receive the generated values and that returned Loader snapshots agree. History tests cover default-only changes/removal versus structural changes.

#151 owns Bootstrap/Downlink behavior. Shared compiler, storage and generated fixtures require integration with whichever branch merges first; this feature must not modify Bootstrap scheduling or cursor contracts. #158 result reuse is unrelated. This assignment prepares and reviews documents only. A separately assigned implementation agent must run affected tests and the full host gate, review final changes and follow its handoff through PR/CI/merge. No implementation is dispatched by this preparation task.
