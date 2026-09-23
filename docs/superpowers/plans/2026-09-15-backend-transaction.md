# `backend.transaction` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `backend.transaction(async ({ tx, notify }) => …)` to the server SDK so writes outside a handler are published and wake live subscribers after commit; remove the `backend.notify(tx, …)` shortcut; migrate every caller; document.

**Architecture:** `createBackend` in `packages/server/index.mts` already has `run(operation)` which opens the adapter transaction, binds a `Session`, runs the completion check and wakes touched channels after commit. `backend.transaction` reuses `run`. The throwaway-session fallback in `publish` goes away. No Rust change.

**Tech Stack:** TypeScript (`packages/server/index.mts`, `.mts` examples), Node test runner with a temporary PostgreSQL cluster (`integration/persistence/server/run.sh`), e2e runner, Markdown docs.

**Spec:** `docs/superpowers/specs/2026-09-15-backend-transaction.md`

## Global Constraints

- Work only in this worktree (`.worktrees/notify-50-transaction`, branch `codex/notify-50-transaction`). Never touch the main checkout.
- Setup once before any JS suite: `npm ci` at the root, then `bash scripts/build.sh` (installs `packages/server` and `packages/client-js` deps, builds `bindings/node/axton-node.node`). Read `docs/engineering/testing/running.md` and `docs/engineering/testing/end-to-end.md` for the exact runner commands; the persistence runner is `bash integration/persistence/server/run.sh`.
- Keep `bindTransaction` and its four methods unchanged.
- Single object argument for the new callback; the type is `TransactionCall<Tx>`.
- Commit messages end with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

---

### Task 1: Add `backend.transaction`, remove the shortcut

**Files:**
- Modify: `packages/server/index.mts` (`NotifyArgs` area near line 196; `publish` near line 640; `bindTransaction` and `run` near lines 673–720; the `api` object near line 730–770)
- Test: `integration/persistence/server/runtime.test.mjs`

**Interfaces:**
- Produces: `export interface TransactionCall<Tx> { tx: Tx; notify(args: NotifyArgs): Promise<void>; }` and `backend.transaction<R>(body: (call: TransactionCall<T>) => Promise<R>): Promise<R>`.
- Removes: `backend.notify(tx, args)`.
- Consumes (in tests): `backend.onCommitted(scope, wake)` (already on the api object), `pull`, `head`, `write`, `count`, `openSocket`, `nextMessage`, `delay` helpers already defined in `runtime.test.mjs`.

- [x] **Step 1: Write the failing tests**

Append to `integration/persistence/server/runtime.test.mjs` (same terse style as the file; check how an existing test reads a pull result, e.g. `(await pull('shared',56)).toCursor`, and how `frames[1].changes.at(-1)` is shaped in `live transport negotiates…`):

```js
test('backend.transaction publishes in the application transaction and wakes after commit',async()=>{
 let woke=0;const unsubscribe=backend.onCommitted('shared',()=>{woke++;});
 const from=(await pull('shared',0)).toCursor;
 const result=await backend.transaction(async({tx,notify})=>{await write(tx,'tx-1','via transaction');await notify({channel:'shared',records:[{model:'Task',identity:{id:'tx-1'}}]});return 'done';});
 assert.equal(result,'done');await delay(0);assert.equal(woke,1,'one wake after commit');
 const page=await pull('shared',from);assert.ok(page.changes.some(c=>c.identity.id==='tx-1'&&c.state.title==='via transaction'),JSON.stringify(page));
 unsubscribe();
});
test('backend.transaction rolls back a failing body and wakes nobody',async()=>{
 let woke=0;const unsubscribe=backend.onCommitted('shared',()=>{woke++;});
 const before=await head('shared');
 await assert.rejects(()=>backend.transaction(async({tx,notify})=>{await write(tx,'tx-rollback','never');await notify({channel:'shared',records:[{model:'Task',identity:{id:'tx-rollback'}}]});throw new Error('cancel');}),/cancel/);
 await delay(0);assert.equal(woke,0);assert.equal(await head('shared'),before);
 assert.equal((await db.$queryRawUnsafe("SELECT count(*) AS count FROM business_task WHERE id='tx-rollback'"))[0].count,0n);
 unsubscribe();
});
test('backend.transaction refuses to commit an unawaited notify',async()=>{
 const before=await head('shared');
 await assert.rejects(()=>backend.transaction(async({tx,notify})=>{await write(tx,'tx-unawaited','never');void notify({channel:'shared',records:[{model:'Task',identity:{id:'tx-unawaited'}}]});}),/unawaited/);
 assert.equal(await head('shared'),before);
});
test('backend.transaction wakes a connected live subscriber without reconnect',async()=>{
 const server=await backend.listen({port:0});const port=Number(new URL(server.url).port);
 const socket=await openSocket(port);const frames=[];socket.addEventListener('message',event=>frames.push(JSON.parse(String(event.data))));
 socket.send(JSON.stringify({type:'subscribe',scopes:['shared'],models:{Task:1}}));while(frames.length<1)await delay(5);
 await backend.transaction(async({tx,notify})=>{await write(tx,'tx-live','live via transaction');await notify({channel:'shared',records:[{model:'Task',identity:{id:'tx-live'}}]});});
 while(frames.length<2)await delay(5);
 assert.deepEqual(frames[1].changes.at(-1).identity,{id:'tx-live'});assert.deepEqual(frames[1].changes.at(-1).state,{title:'live via transaction'});
 socket.close();await new Promise(resolve=>socket.addEventListener('close',resolve,{once:true}));await server.close();
});
test('the unbound notify shortcut is gone and publishing on an unbound transaction is refused',async()=>{
 assert.equal(backend.notify,undefined);
 await assert.rejects(()=>db.$transaction(tx=>backend.bindTransaction(tx).notify({channel:'shared',records:[]}).then(()=>{})),/./);
});
```

Note on the last test: `bindTransaction(tx).notify` on a bound session is fine; what must be refused is a publish on a transaction that was never bound. There is no public entry for that after the shortcut is removed, so replace the second assertion with a check that `bindTransaction` still works end to end if you cannot reach the unbound path from a public API:

```js
 const after=await db.$transaction(async tx=>{const session=backend.bindTransaction(tx);try{await session.notify({channel:'shared',records:[{model:'Task',identity:{id:'bound-still-works'}}]});await session.assertCommittable();return session.afterCommit();}finally{session.close();}});
 after();
```

- [x] **Step 2: Run the suite to verify they fail**

Run: `bash integration/persistence/server/run.sh`
Expected: the five new tests FAIL (`backend.transaction is not a function`; `backend.notify` is defined). Existing tests PASS.

- [x] **Step 3: Add the type, the helper, `transaction`, and remove the shortcut**

In `packages/server/index.mts`:

(a) After `export type NotifyArgs = { … };` add:

```ts
/** What `backend.transaction` hands its body: the application transaction and the external notify bound to it. */
export interface TransactionCall<Tx> {
  tx: Tx;
  /** Reports a business change made outside a handler: every record gets a new stamp and the channel an invalidation, inside `tx`. Await it; a pending notify fails the transaction. */
  notify(args: NotifyArgs): Promise<void>;
}
```

(b) In `publish`, replace

```ts
    const session = sessions.get(tx) ?? new Session();
```

with

```ts
    const session = sessions.get(tx);
    if (!session)
      return Promise.reject(
        new Error(
          "transaction not bound: use backend.transaction or bindTransaction",
        ),
      );
```

(c) Just above `const bindTransaction = (tx: T) => {` add:

```ts
  const notifyIn =
    (tx: T) =>
    ({ channel, records }: NotifyArgs): Promise<void> =>
      publish(
        tx,
        records.map((record) => toRef(record, "notify")),
        [channel],
      ).then(() => undefined);
```

and in `bindTransaction` replace the `notify: ({ channel, records }: NotifyArgs) => publish(…)` member with `notify: notifyIn(tx),`. Keep the doc comment above it.

(d) Just below `run` add:

```ts
  /**
   * Runs `body` in one application transaction with the external notify bound
   * to it. After the adapter commits, the live subscribers of every channel
   * notified are woken; a failure rolls back and wakes nobody. Not for use
   * inside a handler, which already has a transaction and `publish`.
   */
  const transaction = <R,>(
    body: (call: TransactionCall<T>) => Promise<R>,
  ): Promise<R> => run((tx) => body({ tx, notify: notifyIn(tx) }));
```

(e) In the `api` object, delete the `notify: (tx: T, args: NotifyArgs) => publish(…)` member and its doc comment, and add `transaction,` next to `bindTransaction,`.

- [x] **Step 4: Typecheck the package**

Run: `cd packages/server && npx tsc --noEmit -p . ; cd -` (if the package has no `tsconfig.json`, run `npx tsc --noEmit --target ES2022 --module NodeNext --moduleResolution NodeNext --strict --skipLibCheck --allowImportingTsExtensions packages/server/index.mts` from the root).
Expected: no errors. A `run` signature mismatch means `run`'s `operation` is typed `(tx: T, session: Session) => Promise<R>`; passing a one-argument function is fine in TypeScript.

- [x] **Step 5: Run the persistence suite**

Run: `bash integration/persistence/server/run.sh`
Expected: the five new tests PASS; every existing test that called `backend.notify(tx, …)` now FAILS with `backend.notify is not a function`. That is Task 2. If `backend.transaction wakes a connected live subscriber…` times out, compare with `live transport negotiates…`: the subscribe frame and the drain shape are identical; fix the assertion, not the wake.

- [x] **Step 6: Commit**

```bash
git add packages/server/index.mts integration/persistence/server/runtime.test.mjs docs/superpowers/specs/2026-09-15-backend-transaction.md docs/superpowers/plans/2026-09-15-backend-transaction.md
git commit -m "Server SDK: backend.transaction publishes and wakes after commit; drop the notify shortcut (#50)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Migrate the persistence tests off the shortcut

**Files:**
- Modify: `integration/persistence/server/runtime.test.mjs` (every remaining `backend.notify(tx,` call; find them with `grep -n "backend.notify(" integration/persistence/server/runtime.test.mjs`)

**Interfaces:**
- Consumes: `backend.transaction(async ({ tx, notify }) => …)` from Task 1.

- [x] **Step 1: Rewrite each call**

Pattern, for a call that only publishes:

```js
// before
await db.$transaction(tx=>backend.notify(tx,{channel:'snapshot',records:[{model:'Task',identity:{id:'a'}}]}));
// after
await backend.transaction(({notify})=>notify({channel:'snapshot',records:[{model:'Task',identity:{id:'a'}}]}));
```

For a call that writes and publishes:

```js
// before
await db.$transaction(async tx=>{await tx.$executeRawUnsafe("DELETE FROM business_task WHERE id='a'");await backend.notify(tx,{channel:'shared',records:[{model:'Task',identity:{id:'a'}}]});});
// after
await backend.transaction(async({tx,notify})=>{await tx.$executeRawUnsafe("DELETE FROM business_task WHERE id='a'");await notify({channel:'shared',records:[{model:'Task',identity:{id:'a'}}]});});
```

For the rollback test (`publication rollback uses user transaction and rejects unregistered models`): the body that throws `cancel` and the body naming `Unknown` both become `backend.transaction` bodies; the expected rejections (`/cancel/`, `/unregistered loader/`) do not change.

For the 51-row loop that passes `{timeout:20000}` to `db.$transaction`: try `backend.transaction` first. If the adapter's default timeout fails it, keep that one call on `db.$transaction` with `backend.bindTransaction(tx)` (`notify`, `assertCommittable`, `afterCommit`, `close`, then call the returned hook), and say so in the PR body.

Tests whose subject is `bindTransaction` (`pending unawaited publication prevents outer transaction commit`, `external transaction binding retains swallowed publication failure…`, the bound sections of `live transport negotiates…`, the `versioned` backend test near the end) stay on `bindTransaction`.

- [x] **Step 2: Run the suite**

Run: `bash integration/persistence/server/run.sh`
Expected: all PASS. Record the passed count.

- [x] **Step 3: Commit**

```bash
git add integration/persistence/server/runtime.test.mjs
git commit -m "Persistence tests: publish through backend.transaction (#50)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Migrate the fixture servers, the To-do seed and the e2e tests

**Files:**
- Modify: `integration/e2e/fixtures/round-trip/server.mts` (`initialize`)
- Modify: `integration/platform/react-native/server.mts` (seed block after the `CREATE TABLE IF NOT EXISTS "Entry"` statement)
- Modify: `examples/todo/seed.mts` (`seed`), `examples/todo/server.mts` (the `seed(tx, backend)` call near line 130)
- Modify: `integration/e2e/parity.test.mjs` (`reseed`), `integration/e2e/round-trip.test.mjs` (the 55-row seed)

- [x] **Step 1: Fixture server**

In `integration/e2e/fixtures/round-trip/server.mts`, replace

```ts
      await db.$transaction(async (tx) => {
        await tx.entry.upsert({
          where: { id: "entry-1" },
          create: { id: "entry-1", text: "Hello from the server" },
          update: {},
        });
        await backend.notify(tx, {
          channel: "book:demo",
          records: [{ model: "Entry", identity: { id: "entry-1" } }],
        });
      });
```

with

```ts
      await backend.transaction(async ({ tx, notify }) => {
        await tx.entry.upsert({
          where: { id: "entry-1" },
          create: { id: "entry-1", text: "Hello from the server" },
          update: {},
        });
        await notify({
          channel: "book:demo",
          records: [{ model: "Entry", identity: { id: "entry-1" } }],
        });
      });
```

- [x] **Step 2: React Native server**

Same substitution in `integration/platform/react-native/server.mts` (its `create` has `note: null`; keep it).

- [x] **Step 3: To-do seed**

In `examples/todo/seed.mts`, change the function to open the transaction itself:

```ts
/**
 * Creates the demo users and tasks when they are missing and publishes them on
 * `todo:demo` in one backend transaction, so connected phones are woken once
 * it commits. Existing rows are left untouched, so an ordinary restart never
 * resets edits made through the app.
 */
export async function seed(backend: Backend): Promise<void> {
  await backend.transaction(async ({ tx, notify }) => {
    for (const user of SEED_USERS)
      await tx.user.upsert({ where: { id: user.id }, create: user, update: {} });
    for (const todo of SEED_TODOS)
      await tx.todo.upsert({ where: { id: todo.id }, create: todo, update: {} });
    await notify({
      channel: CHANNEL,
      records: [
        ...SEED_USERS.map((user) => ({ model: "User", identity: { id: user.id } })),
        ...SEED_TODOS.map((todo) => ({ model: "Todo", identity: { id: todo.id } })),
      ],
    });
  });
}
```

Remove the now-unused `Tx` alias only if nothing else in the file uses it (`Backend` still needs `createBackend<Tx>`; keep `Tx` if it is referenced there). In `examples/todo/server.mts`, replace `await db.$transaction((tx) => seed(tx, backend));` with `await seed(backend);`. Grep `examples/todo` for other `seed(` callers and `Tx` imports from `seed.mts`.

- [x] **Step 4: e2e tests**

`integration/e2e/parity.test.mjs`, `reseed`:

```js
async function reseed(app){
 await app.backend.transaction(async({tx,notify})=>{
  await tx.entry.update({where:{id:'entry-1'},data:{text:INITIAL}});
  await notify({channel:'book:demo',records:[{model:'Entry',identity:{id:'entry-1'}}]});
 });
 assert.equal((await app.db.entry.findUnique({where:{id:'entry-1'}})).text,INITIAL);
}
```

`integration/e2e/round-trip.test.mjs`, the 55-row seed:

```js
  await app.backend.transaction(async({tx,notify})=>{
   for(let i=0;i<55;i++)await tx.entry.upsert({where:{id:`paged-${i}`},create:{id:`paged-${i}`,text:`record ${i}`},update:{text:`record ${i}`}});
   await notify({channel:'book:demo',records:Array.from({length:55},(_,i)=>({model:'Entry',identity:{id:`paged-${i}`}}))});
  });
```

- [x] **Step 5: Confirm nothing else calls the shortcut**

Run: `grep -rn "backend.notify(\|\.notify(tx" --include='*.mts' --include='*.mjs' --include='*.ts' . | grep -v node_modules | grep -v "^./.worktrees" | grep -v "^./target"`
Expected: no hits outside `bindTransaction(...).notify` session calls.

- [x] **Step 6: Typecheck and run e2e**

Typecheck the touched `.mts` files the way the repo does (look for a `typecheck` or `check` script in the root `package.json`, `examples/todo/package.json` and `integration/platform/react-native/package.json`; if none, use `npx tsc --noEmit -p <dir>` where a `tsconfig.json` exists). The React Native server cannot be run here; typechecking it is the evidence.

Run the e2e suite with the command in `docs/engineering/testing/end-to-end.md` (it creates a temporary PostgreSQL cluster). Expected: PASS, including `parity` and `round-trip`.

- [x] **Step 7: Commit**

```bash
git add integration/e2e examples/todo integration/platform/react-native/server.mts
git commit -m "Examples and fixtures: seed through backend.transaction (#50)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `docs/engineering/architecture/server/engine/notify.md` (§3 table, §6, new §9, §11)
- Modify: `website/docs/backend/api.md` ("Background writes"), `website/docs/backend/setup.md` ("Background jobs"), `website/docs/concepts.md` (the "Background jobs can also publish…" paragraph), `website/docs/api-index.md` (the `backend.notify` row)
- Modify: `docs/engineering/testing/components/server.md` (Notify row's last column), `docs/engineering/testing/integration/persistence.md` (add a row), `docs/engineering/testing/end-to-end.md` (the sentence about the shortcut), `docs/engineering/testing/review.md` (item 1)

- [x] **Step 1: `notify.md`**

§3 table: replace the two "application code" rows with

```
| application code | `backend.transaction(async ({tx, notify}) => …)` | those records, as a business change made outside a handler | one new stamp per record, shared by every channel named |
| application code that owns its transaction (advanced) | `bindTransaction(tx).notify({channel, records})` | as above | as above |
```

§6: replace the sentence starting `Outside a push with \`bindTransaction\`:` with

```
Outside a push with `backend.transaction`: the framework opens the application transaction and binds a session → the body writes and awaits `notify` (stamps advance, invalidations written) → the body returns → completion check → commit → the framework wakes the touched channels. With `bindTransaction` the application performs the last three steps itself: `assertCommittable` → commit → call the function returned by `afterCommit()`.
```

Add a section `## 9. Architecture Decisions` between §6 and §10:

```
## 9. Architecture Decisions

**External writes go through `backend.transaction` (decided in [#50](https://github.com/zanminwang/axton/issues/50), implemented 2026-09-15).** The framework owns the transaction, the completion check and the after-commit wake, so an application cannot publish without waking. Business writes and publications share one transaction; a failure rolls back both and wakes nobody. `bindTransaction` remains for an application whose framework already owns the transaction. The unbound `backend.notify(tx, …)` shortcut, which published without a wake set, was removed. Cross-process wakes stay with [#62](https://github.com/zanminwang/axton/issues/62). Evidence: [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `backend.transaction publishes in the application transaction and wakes after commit`, `backend.transaction rolls back a failing body and wakes nobody`, `backend.transaction refuses to commit an unawaited notify`, `backend.transaction wakes a connected live subscriber without reconnect`.
```

§11: delete the paragraph starting `**Problem: the shortcut \`backend.notify(tx, …)\` never wakes live subscribers.**`. Keep the accepted limitation and the potential risk. Also update the "Code:" line in §5 to name `transaction` next to `Session.touched` and `WakeHub`.

- [x] **Step 2: Website docs**

`website/docs/backend/api.md`, "Background writes": replace the section body (up to "## Extension points") with

````
Writes outside handlers have no readback and no receipt; they reach clients only through channels. Run them through `backend.transaction`: the framework opens the application transaction, `notify` advances the stamp of every record named and publishes them to the channel inside it, and once the transaction commits the framework wakes the live subscribers of those channels.

```ts
await backend.transaction(async ({ tx, notify }) => {
  await tx.entry.update({ where: { id: 'entry-1' }, data: { text: 'From a job' } });
  await notify({
    channel: 'book:demo', records: [Entry({ id: 'entry-1' })],
  });
});
```

`tx` is the transaction of the `Database<Tx>` adapter passed to `createBackend`, and `Entry` is the generated reference function. The body's return value is returned. Await every `notify`; a pending notify when the body returns fails the transaction. If the body throws, the transaction rolls back and nobody is woken; the error propagates so the adapter can retry serialization failures. Do not call `backend.transaction` from a handler: a handler already has a transaction and publishes with `publish`.

| `TransactionCall<Tx>` member | Contract |
| --- | --- |
| `tx` | The application transaction; write business data through it |
| `notify(args)` | Await the stamps and the publication in `tx`; `args` is `NotifyArgs`, `{ channel: string, records: readonly (RecordRef | object)[] }` |

Unlike a handler's `publish`, `notify` is asynchronous and allocates a new stamp per record on every call, because it is the only place the change is reported. Wakeups are process-local; distributed wake delivery needs additional application infrastructure.

### Externally owned transactions

When your framework already owns the transaction and AXTON cannot open it, bind that transaction instead and perform the completion and wake steps yourself:

```ts
const afterCommit = await database.transaction(async tx => {
  const session = backend.bindTransaction(tx);
  try {
    await tx.entry.update({ where: { id: 'entry-1' }, data: { text: 'From a job' } });
    await session.notify({
      channel: 'book:demo', records: [Entry({ id: 'entry-1' })],
    });
    await session.assertCommittable();
    return session.afterCommit();
  } finally {
    session.close();
  }
});
afterCommit();
```

The transaction runner must resolve only after committing. Never invoke the commit callback if the transaction fails.

| Bound-session method | Contract |
| --- | --- |
| `notify(args)` | Await the stamps and the publication in the supplied transaction |
| `assertCommittable()` | Await/check pending work; failure must abort the transaction |
| `afterCommit()` | Capture a zero-argument wakeup callback; call it only after the database commits |
| `close()` | Release the bound session, including on rollback |
````

`website/docs/backend/setup.md`, "Background jobs": replace the paragraph and the code block with

````
Outside a Handler there is no readback and no receipt, so a change must be published to reach clients. Use `backend.transaction`; it advances the stamp of every record named, publishes it to the channel inside the same transaction as your writes, and wakes live subscribers after commit:

```ts
await backend.transaction(async ({ tx, notify }) => {
  await tx.entry.update({ where: { id: 'entry-1' }, data: { text: 'From a job' } });
  await notify({ channel: 'book:demo', records: [Entry({ id: 'entry-1' })] });
});
```

If your framework already owns the transaction, see [externally owned transactions](api.md#externally-owned-transactions).
````

`website/docs/concepts.md`: replace the "Background jobs can also publish…" paragraph with

```
Background jobs publish through `backend.transaction`, which runs the job's writes and its publication in one application transaction, advances the stamps of the records it names, and wakes live subscribers once the transaction commits.
```

and the following sentence `The [backend SDK guide](backend/setup.md) shows both registration and transaction-bound publication.` with `The [backend SDK guide](backend/setup.md) shows registration and background publication.`

`website/docs/api-index.md`: replace the `backend.notify` row with

```
| `backend.transaction`, `TransactionCall`, `NotifyArgs` | Write and publish outside a handler; subscribers wake after commit | [Background writes](backend/api.md#background-writes) |
| `backend.bindTransaction` | Publish inside a transaction your framework owns | [Externally owned transactions](backend/api.md#externally-owned-transactions) |
```

If `api-index.md` has an "Advanced interfaces" table, put the `bindTransaction` row there instead.

- [x] **Step 3: Testing docs**

`docs/engineering/testing/components/server.md`, Notify row, last column: replace the text about the shortcut with `none; external writes are covered by the `backend.transaction` tests in [Persistence](../integration/persistence.md).`

`docs/engineering/testing/integration/persistence.md`: add a row in the same table style:

```
| External writes: `backend.transaction` publishes in the application transaction and wakes only after commit; a failing body rolls back and wakes nobody; an unawaited notify cannot commit; a connected live subscriber is woken without reconnect ([Notify §9](../../architecture/server/engine/notify.md#9-architecture-decisions)) | `backend.transaction publishes in the application transaction and wakes after commit`, `backend.transaction rolls back a failing body and wakes nobody`, `backend.transaction refuses to commit an unawaited notify`, `backend.transaction wakes a connected live subscriber without reconnect` | covered | Executed 2026-09-15 by `bash integration/persistence/server/run.sh` (<count> passed). |
```

`docs/engineering/testing/end-to-end.md`: replace `except the wiring itself and the \`backend.notify(tx, …)\` shortcut used by the fixture server and the To-do seed, which relies on catch-up rather than a wake ([Notify §11](../architecture/server/engine/notify.md))` with `except the wiring itself`.

`docs/engineering/testing/review.md`, item 1: remove `the \`backend.notify(tx, …)\` shortcut that never wakes subscribers ([#50](https://github.com/zanminwang/axton/issues/50), [Notify §11](../architecture/server/engine/notify.md));` and fix the sentence.

- [x] **Step 4: Sweep**

Run: `grep -rn "backend.notify\|bindTransaction\|afterCommit" docs website README.md packages/server/README.md examples --include='*.md'`
Expected: every remaining mention is the advanced path or history. Check that every relative link and anchor you wrote resolves (`#externally-owned-transactions` must match the heading in `api.md`).

- [x] **Step 5: Commit**

```bash
git add docs website README.md packages/server/README.md
git commit -m "Docs: backend.transaction for external writes (#50)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Verify and open the pull request

- [ ] **Step 1: Run everything touched**

```bash
bash integration/persistence/server/run.sh
<e2e runner command from docs/engineering/testing/end-to-end.md>
```

plus the typechecks from Task 3 Step 6. Expected: all PASS. Record counts and which commands ran.

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin codex/notify-50-transaction
gh pr create --base main --title "Server SDK: backend.transaction wakes live subscribers after external writes (#50)" --body "$(cat <<'EOF'
Closes #50.

`backend.transaction(async ({ tx, notify }) => …)` runs writes and publications in one application transaction and wakes the live subscribers of the touched channels after commit. A failing body rolls back and wakes nobody; an unawaited `notify` cannot commit. The unbound `backend.notify(tx, …)` shortcut, which published without a wake set, is removed; `bindTransaction` stays as the advanced path for externally owned transactions. Fixture servers, the To-do seed and the e2e tests migrated.

Spec: `docs/superpowers/specs/2026-09-15-backend-transaction.md`.

Evidence: `bash integration/persistence/server/run.sh` (<count> passed), e2e runner (<result>), typechecks for `examples/todo` and `integration/platform/react-native` (<result>). The React Native server was typechecked, not run on a simulator.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Report the PR URL.
