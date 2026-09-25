import test, { after, before } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { GeneratedClient } from "./client.ts";
import { createFixture } from "./backend-fixture.ts";
import { GeneratedClient as EvolvedClient } from "./evolved/client.ts";
import { createBackend as createEvolvedBackend, devAuth, type Handlers as EvolvedHandlers, type Loaders as EvolvedLoaders } from "./evolved/backend.ts";
import { pg, type PgClient } from "../../packages/postgres/index.mts";

const fixture = await createFixture();
let url: string;
before(async () => { await fixture.initialize(); url = (await fixture.listen()).url; });
after(async () => { await fixture.close(); });
const server = () => ({ url, token: "alice" });
const wait = async (predicate: () => Promise<boolean>, label: string) => {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw Error(`Timed out waiting for ${label}`);
};
const post = async (kind: "mutations" | "pull", body: string, target = url) => {
  const response = await fetch(`${target}/sync/${kind}`, { method: "POST", headers: { authorization: "Bearer alice", "content-type": "application/json" }, body });
  if (response.status !== 200) throw Error(`HTTP ${response.status}: ${await response.text()}`);
  return response.json();
};

test("generated Add, Update, Delete, SendEmail, and Search cross native SQLite and PostgreSQL", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-action-generated-"));
  const path = join(directory, "client.sqlite");
  let client: GeneratedClient | undefined;
  try {
    client = await GeneratedClient.open({ path });
    const initial = await client.actions.addTodo({ todo: { id: "main", title: "  first  " } });
    assert.equal(initial.status, "pending");
    assert.equal((await client.models.todo.get({ id: "main" }))?.title, "  first  ");
    assert.equal((await client.syncState()).pending, 1);
    await client.close();
    client = await GeneratedClient.open({ path, server: server() });
    await wait(async () => (await client!.syncState()).pending === 0, "offline AddTodo after SQLite reopen");
    assert.equal((await client.models.todo.get({ id: "main" }))?.title, "first");
    assert.deepEqual((await fixture.pool.query("SELECT id,title FROM action_e2e_todo WHERE id='main'")).rows, [{ id: "main", title: "first" }]);

    const search = await client.actions.call.searchTodos({ query: null });
    assert.equal(search.count, 1);
    assert.deepEqual(search.labels, ["main"]);
    assert.equal(search.hint, null);
    assert.equal(search.todos[0]?.title, "first");
    assert.equal(search.first?.title, "first");
    await client.connection!.pause();
    await client.actions.sendEmail({ to: "test@example.invalid", subject: "Queued", body: "Offline" });
    assert.equal((await client.syncState()).pending, 1, "ordinary-only Action enqueues offline without a Model target");
    await client.close();
    client = await GeneratedClient.open({ path });
    assert.equal((await client.syncState()).pending, 1, "ordinary-only Action survived SQLite reopen");
    await client.connect(server());
    await wait(async () => (await client!.syncState()).pending === 0, "offline SendEmail settlement");
    const directMail = await client.actions.call.sendEmail({ to: "test@example.invalid", subject: "Direct", body: "Immediate" });
    assert.match(directMail.messageId, /^[0-9]+$/);
    const durableMail = await client.actions.sendEmail({ to: "test@example.invalid", subject: "Durable", body: "Awaited" });
    const durableOutcome = await durableMail.wait();
    assert.equal(durableOutcome.error, null);
    assert.match(durableOutcome.result!.messageId, /^[0-9]+$/);
    assert.notEqual(durableOutcome.result!.messageId, directMail.messageId);
    await client.connection!.pause();
    const lostMail = await client.actions.sendEmail({ to: "test@example.invalid", subject: "Replay", body: "Once" });
    const frozenMail = await client.client.freeze();
    assert.ok(frozenMail);
    const firstMail = await post("mutations", frozenMail);
    const handledMail = fixture.handlerCalls;
    const replayedMail = await post("mutations", frozenMail);
    assert.deepEqual(replayedMail.completions, firstMail.completions, "cached ordinary output retains messageId");
    assert.equal(fixture.handlerCalls, handledMail, "ordinary side effect was not repeated");
    await client.client.acknowledge(JSON.parse(frozenMail).batchSequence, replayedMail);
    const replayedOutcome = await lostMail.wait();
    assert.equal(replayedOutcome.error, null);
    assert.equal(replayedOutcome.result!.messageId, firstMail.completions[0].outcome.result.messageId);
    await client.connection!.resume();
    assert.deepEqual((await fixture.pool.query("SELECT recipient,subject,body FROM action_e2e_outbox ORDER BY id")).rows, [
      { recipient: "test@example.invalid", subject: "Queued", body: "Offline" },
      { recipient: "test@example.invalid", subject: "Direct", body: "Immediate" },
      { recipient: "test@example.invalid", subject: "Durable", body: "Awaited" },
      { recipient: "test@example.invalid", subject: "Replay", body: "Once" },
    ]);
    const update = await client.actions.updateTodo({ todo: { id: "main", title: "  revised  " } });
    assert.equal((await update.wait()).error, null);
    assert.equal((await client.models.todo.get({ id: "main" }))?.title, "revised");
    const deleted = await client.actions.deleteTodo({ todo: { id: "main" } });
    assert.equal((await deleted.wait()).error, null);
    assert.equal(await client.models.todo.get({ id: "main" }), null);
    assert.deepEqual((await fixture.pool.query("SELECT id FROM action_e2e_todo WHERE id='main'")).rows, []);
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});

test("direct result retains Loader snapshot while independent durable optimism replays", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-action-direct-"));
  let client: GeneratedClient | undefined;
  try {
    client = await GeneratedClient.open({ path: join(directory, "client.sqlite"), server: server() });
    const created = await client.actions.addTodo({ todo: { id: "direct", title: "A" } });
    assert.equal((await created.wait()).error, null);
    await client.connection!.pause();
    await client.channels.subscribe("todos:demo");
    const pending = await client.actions.updateTodo({ todo: { id: "direct", title: "B" } });
    assert.equal(pending.status, "pending");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "B");
    const result = await client.actions.call.searchTodos({ query: "A" });
    assert.equal(result.todos[0]?.title, "A", "result is the committed Loader snapshot");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "B", "direct authority replays the independent pending edit");
    assert.equal((await client.syncState()).pending, 1, "direct call did not drain the durable queue");
    const later = await client.actions.updateTodo({ todo: { id: "direct", title: "C" } });
    const frozen = await client.client.freeze();
    assert.ok(frozen);
    assert.equal(JSON.parse(frozen).mutations.length, 2, "both invocations are frozen in one batch");
    const receipt = await post("mutations", frozen);
    await client.client.acknowledge(JSON.parse(frozen).batchSequence, receipt);
    const firstOutcome = await pending.wait();
    const secondOutcome = await later.wait();
    assert.equal(firstOutcome.error, null);
    assert.equal(secondOutcome.error, null);
    assert.equal(firstOutcome.result.todo.title, "B", "first invocation keeps its own Loader snapshot");
    assert.equal(secondOutcome.result.todo.title, "C", "second invocation keeps its later Loader snapshot");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "C");
    const beforePage = await client.syncState();
    const page = await post("pull", JSON.stringify({ cursors: { "todos:demo": beforePage.cursors["todos:demo"] ?? 0 }, models: { Todo: 1 } }));
    await client.client.applyPull(page);
    const afterPage = await client.syncState();
    assert.ok((afterPage.cursors["todos:demo"] ?? 0) > (beforePage.cursors["todos:demo"] ?? 0), "page advances the cursor after the receipt");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "C", "the later page cannot regress receipt authority");
    assert.equal(result.todos[0]?.title, "A", "result does not change after later settlement");
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});

test("store selects which Search outputs update local Models on both routes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-action-store-"));
  let client: GeneratedClient | undefined;
  const local = (id: string) => client!.models.todo.get({ id });
  const localStamp = async (id: string) =>
    (await client!.readSql("SELECT stamp FROM axton_record WHERE model='Todo' AND identity LIKE ?", [`%${id}%`]))[0]?.stamp ?? null;
  const serverStamps = async (id: string) =>
    (await fixture.pool.query("SELECT stamp FROM axton_record WHERE model='Todo' AND identity_key LIKE $1", [`%${id}%`])).rows.length;
  try {
    await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('store-a','storeq a'),('store-b','storeq b')");
    client = await GeneratedClient.open({ path: join(directory, "client.sqlite"), server: server() });
    let notifications = 0;
    const stop = client.models.todo.watch({}, () => { notifications++; });
    await new Promise((resolve) => setTimeout(resolve, 30));
    const settled = notifications;

    // Direct store:false returns full Loader snapshots and stores nothing.
    const unstored = await client.actions.call.searchTodos({ query: "storeq" }, { store: false });
    assert.deepEqual(unstored.todos.map((todo) => todo.title), ["storeq a", "storeq b"]);
    assert.equal(unstored.first?.title, "storeq a");
    assert.equal(await local("store-a"), null);
    assert.equal(await localStamp("store-a"), null);
    assert.equal(await serverStamps("store-a"), 0, "no output-only stamp allocation");
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(notifications, settled, "no Model notification for a disabled-only read");

    // Mixed: first is stored, a record only in todos is not.
    const mixed = await client.actions.call.searchTodos({ query: "storeq" }, { store: { todos: false } });
    assert.equal(mixed.todos.length, 2);
    assert.equal((await local("store-a"))?.title, "storeq a", "enabled overlapping output stores its record");
    assert.equal(await local("store-b"), null, "disabled-only record is not stored");
    const stampA = await localStamp("store-a");
    assert.ok(stampA);

    // A cached row stays unchanged when a disabled read returns newer content.
    // Another writer changes the record and advances its stamp.
    await fixture.pool.query("UPDATE action_e2e_todo SET title='storeq a2' WHERE id='store-a'");
    await fixture.pool.query("UPDATE axton_record SET stamp=stamp+1 WHERE model='Todo' AND identity_key LIKE '%store-a%'");
    const newer = await client.actions.call.searchTodos({ query: "storeq" }, { store: false });
    assert.equal(newer.first?.title, "storeq a2", "result is this invocation's Loader snapshot");
    assert.equal((await local("store-a"))?.title, "storeq a", "cached row unchanged");
    assert.equal(await localStamp("store-a"), stampA);

    // Durable store:false behaves the same, through the queue and receipt.
    const durable = await client.actions.searchTodos({ query: "storeq" }, { store: false });
    const outcome = await durable.wait();
    assert.equal(outcome.error, null);
    assert.equal(outcome.result!.todos[1]?.title, "storeq b");
    assert.equal(await local("store-b"), null);
    assert.equal((await local("store-a"))?.title, "storeq a");
    // The default stores every eligible output.
    const stored = await client.actions.searchTodos({ query: "storeq" });
    assert.equal((await stored.wait()).error, null);
    assert.equal((await local("store-a"))?.title, "storeq a2");
    assert.equal((await local("store-b"))?.title, "storeq b");

    // Required mutation reconciliation is never disabled.
    await client.connection!.pause();
    const edit = await client.actions.updateTodo({ todo: { id: "store-a", title: "  edited  " } }, { store: false });
    assert.equal((await local("store-a"))?.title, "  edited  ", "optimistic edit");
    const snapshot = await client.actions.call.searchTodos({ query: "storeq" }, { store: false });
    assert.equal(snapshot.first?.title, "storeq a2", "snapshot A while pending edit B stays local");
    assert.equal((await local("store-a"))?.title, "  edited  ");
    await client.connection!.resume();
    assert.equal((await edit.wait()).error, null);
    assert.equal((await local("store-a"))?.title, "edited", "required authority reconciled the write");
    assert.equal(snapshot.first?.title, "storeq a2", "returned snapshot does not change later");
    stop();
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});

test("committed response loss replays frozen intent and stored result after a channel page arrives first", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-action-replay-"));
  const path = join(directory, "client.sqlite");
  let client: GeneratedClient | undefined;
  let upgraded: EvolvedClient | undefined;
  let evolvedListener: Awaited<ReturnType<ReturnType<typeof createEvolvedBackend<PgClient>>["listen"]>> | undefined;
  try {
    client = await GeneratedClient.open({ path });
    await client.channels.subscribe("todos:demo");
    await client.actions.addTodo({ todo: { id: "replay", title: "  saved  " } });
    const frozen = await client.client.freeze();
    assert.ok(frozen);
    const first = await post("mutations", frozen);
    const page = await post("pull", JSON.stringify({ cursors: { "todos:demo": 0 }, models: { Todo: 1 } }));
    await client.client.applyPull(page);
    const handled = fixture.handlerCalls;
    const loaded = fixture.loaderCalls;
    assert.equal((await client.syncState()).pending, 1, "page authority alone does not complete the Action");
    await client.close();
    await fixture.pool.query("ALTER TABLE action_e2e_todo ADD COLUMN note text");
    let unexpected = 0;
    const forbidden = async (): Promise<never> => { unexpected++; throw Error("cached Action reexecuted"); };
    const handlers: EvolvedHandlers<PgClient> = {
      addTodo: { v1: forbidden, v2: forbidden },
      updateTodo: { v1: forbidden, v2: forbidden },
      deleteTodo: forbidden,
      sendEmail: forbidden,
      searchTodos: { v1: forbidden, v2: forbidden },
    };
    const loaders: EvolvedLoaders<PgClient> = { todo: { v1: forbidden, v2: forbidden } };
    const evolvedBackend = createEvolvedBackend<PgClient>({ database: pg(fixture.pool), authenticate: devAuth(), handlers, loaders });
    evolvedListener = await evolvedBackend.listen({ port: 0 });
    upgraded = await EvolvedClient.open({ path });
    assert.equal(await upgraded.client.freeze(), frozen, "SQLite retained the exact frozen request bytes through nullable schema evolution");
    const replay = await post("mutations", frozen, evolvedListener.url);
    assert.deepEqual(replay.completions, first.completions);
    assert.deepEqual(replay.records, first.records, "frozen v1 read intent keeps its original authority encoding");
    assert.equal(fixture.handlerCalls, handled, "handler was not rerun");
    assert.equal(fixture.loaderCalls, loaded, "Loader was not rerun");
    assert.equal(unexpected, 0, "upgraded handlers and Loaders were never invoked");
    await upgraded.client.acknowledge(JSON.parse(frozen).batchSequence, replay);
    assert.equal((await upgraded.syncState()).pending, 0);
    assert.deepEqual(await upgraded.models.todo.get({ id: "replay" }), { id: "replay", title: "saved", note: null });
  } finally { await client?.close(); await upgraded?.close(); await evolvedListener?.close(); await rm(directory, { recursive: true, force: true }); }
});
