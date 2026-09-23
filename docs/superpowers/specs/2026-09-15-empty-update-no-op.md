# Empty update patches are no-ops

Status: implementation specification. Tracking: [#49](https://github.com/zanminwang/axton/issues/49).

## 1. Decision

An update operation whose patch is empty is a valid, meaningful mutation that changes no field. The user decided on 2026-09-15: "空的就当做没有" (an empty patch is treated as nothing to write).

Consequences:

- `Model.update<>` is a valid slot declaration. It compiles, its `allowedPatchFields` descriptor is `[]`, and the generated input type is `Pick<ModelPatch, never>` (TypeScript) / a patch with no settable field (Dart), exactly as today.
- `Model.update` without a field list on a model whose only fields are its identity resolves to the same empty allowed list and is likewise valid.
- The server decoder accepts an update operation whose `values` object contributes no allowed field. The slot argument handed to the handler is `{ identity, patch: {} }`.
- Everything downstream is unchanged: the record is still a target of the mutation, so the handler runs with that argument, may reject it, the record gets a new stamp, is read back through the loader and lands in the receipt, and is published if the handler publishes. No special case anywhere.
- The client already accepts an empty patch (`validate_patch` and `apply_to_row` in `crates/client/src/mutate.rs` loop over zero keys); nothing changes there.

## 2. What changes

| Layer | Today | After |
| --- | --- | --- |
| Compiler (`crates/compiler/src/validate.rs`) | accepts `Parent.update<>` | unchanged; a test now states this on purpose |
| Server decoder (`crates/server/src/lib.rs`, `decode`) | `if data.is_empty() { return Err(mutation.invalid) }` | branch removed; `patch: {}` is decoded |
| Generated types | `Pick<Patch, never>` | unchanged |
| Docs | `mutations.md` §5 lists "an empty patch" as `mutation.invalid`; §11 records the problem | §5 no longer lists it; §9 records the decision; §11 entry removed; testing docs updated |

`values` must still be a JSON object. An update with `values` missing or not an object stays `mutation.invalid`; a known field outside the allowed list stays `<mutation>.not_allowed`. Unknown fields are still ignored, so `values: { future: 1 }` against a slot that does not allow `future` decodes to `patch: {}` when `future` is not a known field of any retained input snapshot.

## 3. Out of scope

- Whether a no-op update should skip the stamp allocation. It does not; a target is a target. Revisit only if [#12](https://github.com/zanminwang/axton/issues/12) measurements show it matters.
- Any change to [#54](https://github.com/zanminwang/axton/issues/54) slot ordering; that is a separate branch.

## 4. Done when

- [x] `cargo test -p axton-server` has a test decoding an update whose `values` is `{}` to `{ identity, patch: {} }`, and one where every supplied field is unknown decodes to `patch: {}`.
- [x] `cargo test -p axton-compiler` asserts `Parent.update<>` compiles with `allowedPatchFields: []`.
- [x] `docs/engineering/architecture/schema/mutations.md` §5, §9 and §11 match the decision; `docs/engineering/testing/components/schema.md` and `docs/engineering/testing/review.md` no longer call this a defect.
