# Empty update no-op Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an update operation with an empty patch decode to `patch: {}` on the server instead of `mutation.invalid`, and document that `Model.update<>` is valid.

**Architecture:** One branch removed in the server's slot decoder (`decode` in `crates/server/src/lib.rs`); everything downstream already handles a target with an empty patch. The compiler is unchanged; a test pins the accepted shape. Architecture and testing docs move the item from "problem" to "decision".

**Tech Stack:** Rust (cargo workspace), Markdown docs.

**Spec:** `docs/superpowers/specs/2026-09-15-empty-update-no-op.md`

## Global Constraints

- Work only in this worktree (`.worktrees/schema-49-empty-update`, branch `codex/schema-49-empty-update`). Never touch the main checkout.
- Do not change the generated types, the client, the wire format or slot ordering.
- Keep `mutation.invalid` for a missing or non-object `values`, and `<mutation>.not_allowed` for a known field outside the allowed list.
- Run `cargo fmt --all` before each commit. Commit messages end with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

---

### Task 1: Server decodes an empty patch

**Files:**
- Modify: `crates/server/src/lib.rs` (function `decode`, the `if data.is_empty()` block just before `argument["patch"] = Value::Object(data);`)
- Test: `crates/server/tests/runtime.rs`

**Interfaces:**
- Consumes: `axton_server::decode_arguments(config: &Value, body: &Value) -> Result<Value>` and the `config()` helper already in `runtime.rs` (mutation `edit`, slot `task`, `allowedPatchFields: ["title"]`, model `Task` with fields `id`, `title`, `note`).
- Produces: nothing new; behavior only.

- [x] **Step 1: Write the failing tests**

Append to `crates/server/tests/runtime.rs`:

```rust
#[test]
fn empty_patch_decodes_as_a_no_op_update() {
    let args = axton_server::decode_arguments(
        &config(),
        &json!({"name":"edit","operations":[{"model":"Task","op":"update","identity":{"id":"a"},"values":{}}]}),
    )
    .unwrap();
    assert_eq!(args, json!({"task":{"identity":{"id":"a"},"patch":{}}}));
}
#[test]
fn a_patch_of_only_unknown_fields_decodes_as_a_no_op_update() {
    let args = axton_server::decode_arguments(
        &config(),
        &json!({"name":"edit","operations":[{"model":"Task","op":"update","identity":{"id":"a"},"values":{"future":true}}]}),
    )
    .unwrap();
    assert_eq!(args, json!({"task":{"identity":{"id":"a"},"patch":{}}}));
}
#[test]
fn update_values_must_still_be_an_object() {
    let err = axton_server::decode_arguments(
        &config(),
        &json!({"name":"edit","operations":[{"model":"Task","op":"update","identity":{"id":"a"},"values":null}]}),
    )
    .unwrap_err();
    assert_eq!(err.code, "mutation.invalid");
}
```

- [x] **Step 2: Run them to verify the first two fail**

Run: `cargo test -p axton-server --test runtime no_op_update`
Expected: `empty_patch_decodes_as_a_no_op_update` and `a_patch_of_only_unknown_fields_decodes_as_a_no_op_update` FAIL with `called Result::unwrap() on an Err value` whose code is `mutation.invalid`. Run `cargo test -p axton-server --test runtime update_values_must_still_be_an_object` and expect PASS (it pins existing behavior).

- [x] **Step 3: Remove the empty-patch refusal**

In `crates/server/src/lib.rs`, inside `decode`, delete these three lines:

```rust
                    if data.is_empty() {
                        return Err(Error::code("mutation.invalid"));
                    }
```

so the block ends with `argument["patch"] = Value::Object(data);`.

- [x] **Step 4: Run the server tests**

Run: `cargo test -p axton-server`
Expected: all PASS, including the three new tests. If any existing test asserted `mutation.invalid` for an empty patch, it now fails: read it, and if it only pins the old refusal, change its expectation to the decoded `patch: {}`; if it tests something else, report it instead of forcing it green.

- [x] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/server/src/lib.rs crates/server/tests/runtime.rs docs/superpowers/specs/2026-09-15-empty-update-no-op.md docs/superpowers/plans/2026-09-15-empty-update-no-op.md
git commit -m "Server: decode an empty update patch as a no-op (#49)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Compiler test states that `update<>` is accepted on purpose

**Files:**
- Test: `crates/compiler/tests/compiler.rs` (test `relationships_bindings_and_dependency_metadata`, which already compiles `mutation Rename { parent Parent.update<> }`)

**Interfaces:**
- Consumes: the `compile(source: &str) -> Result<Value, String>` helper at the top of `compiler.rs`; the compiled value's `v["mutations"][i]["slots"][j]["allowedPatchFields"]`.

- [x] **Step 1: Add the assertion**

In `relationships_bindings_and_dependency_metadata`, after the existing `assert_eq!(v["prerequisites"][0]["name"], "Uploaded");`, add:

```rust
    // `Parent.update<>` is valid: an empty patch is a no-op update ([#49](https://github.com/zanminwang/axton/issues/49)).
    assert_eq!(v["mutations"][1]["name"], "Rename");
    assert_eq!(
        v["mutations"][1]["slots"][0]["allowedPatchFields"],
        serde_json::json!([])
    );
```

- [x] **Step 2: Run the compiler tests**

Run: `cargo test -p axton-compiler --test compiler relationships_bindings_and_dependency_metadata`
Expected: PASS. If the index or key name differs (for example the descriptor key is not `allowedPatchFields`), print `v["mutations"]` once with `println!("{}", v["mutations"])`, fix the assertion to the real key, and remove the print.

- [x] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/compiler/tests/compiler.rs
git commit -m "Compiler: pin that Model.update<> compiles with an empty allowed list (#49)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Documentation follows the decision

**Files:**
- Modify: `docs/engineering/architecture/schema/mutations.md` (§5 "Decoding on the server", §9, §11)
- Modify: `docs/engineering/testing/components/schema.md` (the "Slot decoding" row)
- Modify: `docs/engineering/testing/review.md` (item 2, the `Model.update<>` sentence)

- [x] **Step 1: Edit `mutations.md` §5**

Replace, in the "Decoding on the server" paragraph:

```
an unknown mutation, a wrong shape, a missing required create field or an empty patch is `mutation.invalid`;
```

with:

```
an unknown mutation, a wrong shape or a missing required create field is `mutation.invalid`; an empty patch decodes to `patch: {}` and the update is a no-op that still stamps, reads back and may publish its record;
```

- [x] **Step 2: Add the decision to §9**

Append to "## 9. Architecture Decisions" (after the deprecation paragraph):

```
**An empty update patch is a no-op, not a refusal (decided 2026-09-15, [#49](https://github.com/zanminwang/axton/issues/49)).** `Model.update<>` is a valid declaration: its allowed list is empty, the generated input has no settable field, and the server decodes the operation to `{identity, patch: {}}`. The record remains a target of the mutation: the handler runs, a stamp is allocated, the loader reads it back into the receipt, and a publication distributes it. No layer special-cases the empty patch. Evidence: [server/tests/runtime.rs](../../../../crates/server/tests/runtime.rs) `empty_patch_decodes_as_a_no_op_update`, `a_patch_of_only_unknown_fields_decodes_as_a_no_op_update`; [compiler/tests/compiler.rs](../../../../crates/compiler/tests/compiler.rs) `relationships_bindings_and_dependency_metadata`.
```

- [x] **Step 3: Remove the §11 entry**

Delete the paragraph starting `**Problem: \`Model.update<>\` compiles but never succeeds.**` from "## 11. Risks and Technical Debt". Keep the adjacent-slot paragraph (it belongs to #54).

- [x] **Step 4: Update the testing docs**

In `docs/engineering/testing/components/schema.md`, in the "Slot decoding" row, replace the sentence

```
`Model.update<>` compiles and always fails on the server: a *defect* in the compiler's acceptance, no test.
```

with

```
An empty patch is a no-op update, covered by `empty_patch_decodes_as_a_no_op_update` ([Mutations §9](../../architecture/schema/mutations.md#9-architecture-decisions)).
```

In `docs/engineering/testing/review.md`, item 2, replace

```
`Model.update<>` compiling but never succeeding ([#49](https://github.com/zanminwang/axton/issues/49)) needs a focused regression with its fix.
```

with

```
An empty update patch is a decided no-op with its regression in `server/tests/runtime.rs` ([#49](https://github.com/zanminwang/axton/issues/49)).
```

- [x] **Step 5: Check the links**

Run: `grep -rn "update<>\|empty patch" docs website/docs --include='*.md'`
Expected: only the §9 decision, the test-doc sentence, and this spec/plan mention them. Fix any leftover claim that an empty patch is refused.

- [x] **Step 6: Commit**

```bash
git add docs/engineering/architecture/schema/mutations.md docs/engineering/testing/components/schema.md docs/engineering/testing/review.md
git commit -m "Docs: empty update patches are no-ops (#49)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Verify and open the pull request

- [x] **Step 1: Full Rust check**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all PASS. Record the exact pass/fail counts for the PR body.

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin codex/schema-49-empty-update
gh pr create --base main --title "Schema and server: empty update patches are no-ops (#49)" --body "$(cat <<'EOF'
Closes #49.

Decision (2026-09-15): an empty update patch is a no-op. `Model.update<>` stays valid; the server decodes an update with no allowed field to `{ identity, patch: {} }` instead of refusing it as `mutation.invalid`. The record remains a target: handler, stamp, readback, receipt and publication are unchanged. `values` missing or non-object is still `mutation.invalid`; a known field outside the allowed list is still `<mutation>.not_allowed`.

Spec: `docs/superpowers/specs/2026-09-15-empty-update-no-op.md`.

Evidence: `cargo test --workspace --locked` (<fill in counts>), `cargo clippy --workspace --all-targets --locked -- -D warnings` clean.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Report the PR URL.
