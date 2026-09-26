import test, { after, before } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { GeneratedClient } from "./client.ts";
import { createFixture } from "./backend-fixture.ts";
import { GeneratedClient as EvolvedClient } from "./evolved/client.ts";
import { createBackend as createEvolvedBackend, devAuth, type Mutations as EvolvedMutations, type Queries as EvolvedQueries, type Loaders as EvolvedLoaders } from "./evolved/backend.ts";
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
/** Records the sync paths the client requests while `body` runs: which delivery path a call took. */
const requestedPaths = async (body: () => Promise<void>) => {
  const original = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = ((input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
    const path = new URL(input instanceof Request ? input.url : String(input)).pathname;
    if (path.startsWith("/sync/")) paths.push(path);
    return original(input, init);
  }) as typeof fetch;
  try { await body(); } finally { globalThis.fetch = original; }
  return paths;
};
const post = async (kind: "mutations" | "pull" | "actions", body: string, target = url) => {
  const response = await fetch(`${target}/sync/${kind}`, { method: "POST", headers: { authorization: "Bearer alice", "content-type": "application/json" }, body });
  if (response.status !== 200) throw Error(`HTTP ${response.status}: ${await response.text()}`);
  return response.json();
};

test("generated Mutations and the Search Query cross native SQLite and PostgreSQL", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-action-generated-"));
  const path = join(directory, "client.sqlite");
  let client: GeneratedClient | undefined;
  try {
    client = await GeneratedClient.open({ path });
    const initial = await client.mutations.addTodo({ todo: { id: "main", title: "  first  " } });
    assert.equal(initial.status, "pending");
    assert.equal((await client.models.todo.get({ id: "main" }))?.title, "  first  ");
    assert.equal((await client.syncState()).pending, 1);
    await client.close();
    client = await GeneratedClient.open({ path, server: server() });
    await wait(async () => (await client!.syncState()).pending === 0, "offline AddTodo after SQLite reopen");
    assert.equal((await client.models.todo.get({ id: "main" }))?.title, "first");
    assert.deepEqual((await fixture.pool.query("SELECT id,title FROM action_e2e_todo WHERE id='main'")).rows, [{ id: "main", title: "first" }]);

    const search = await client.queries.searchTodos({ query: null });
    assert.equal(search.count, 1);
    assert.deepEqual(search.labels, ["main"]);
    assert.equal(search.hint, null);
    assert.equal(search.todos[0]?.title, "first");
    assert.equal(search.first?.title, "first");
    await client.connection!.pause();
    await client.mutations.sendEmail({ to: "test@example.invalid", subject: "Queued", body: "Offline" });
    assert.equal((await client.syncState()).pending, 1, "ordinary-only Mutation enqueues offline without a Model target");
    await client.close();
    client = await GeneratedClient.open({ path });
    assert.equal((await client.syncState()).pending, 1, "ordinary-only Mutation survived SQLite reopen");
    await client.connect(server());
    await wait(async () => (await client!.syncState()).pending === 0, "offline SendEmail settlement");
    const directMail = await client.mutations.call.sendEmail({ to: "test@example.invalid", subject: "Direct", body: "Immediate" });
    assert.match(directMail.messageId, /^[0-9]+$/);
    const durableMail = await client.mutations.sendEmail({ to: "test@example.invalid", subject: "Durable", body: "Awaited" });
    const durableOutcome = await durableMail.wait();
    assert.equal(durableOutcome.error, null);
    assert.match(durableOutcome.result!.messageId, /^[0-9]+$/);
    assert.notEqual(durableOutcome.result!.messageId, directMail.messageId);
    await client.connection!.pause();
    const lostMail = await client.mutations.sendEmail({ to: "test@example.invalid", subject: "Replay", body: "Once" });
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
    const update = await client.mutations.updateTodo({ todo: { id: "main", title: "  revised  " } });
    assert.equal((await update.wait()).error, null);
    assert.equal((await client.models.todo.get({ id: "main" }))?.title, "revised");
    const deleted = await client.mutations.deleteTodo({ todo: { id: "main" } });
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
    // The subscription's origin is the first head its handshake acknowledges
    // (#150), so it is registered and initialized before the changes it must
    // receive; a null boundary is never read as zero and the pull below starts at
    // that committed cursor.
    const subscription = await client.scopes.subscribe("todos:demo");
    await wait(async () => subscription.status.initialization === "ready", "first initialization");
    const created = await client.mutations.addTodo({ todo: { id: "direct", title: "A" } });
    assert.equal((await created.wait()).error, null);
    await client.connection!.pause();
    const pending = await client.mutations.updateTodo({ todo: { id: "direct", title: "B" } });
    assert.equal(pending.status, "pending");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "B");
    const result = await client.queries.searchTodos({ query: "A" });
    assert.equal(result.todos[0]?.title, "A", "result is the committed Loader snapshot");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "B", "direct authority replays the independent pending edit");
    assert.equal((await client.syncState()).pending, 1, "direct call did not drain the durable queue");
    const later = await client.mutations.updateTodo({ todo: { id: "direct", title: "C" } });
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
    assert.equal(typeof beforePage.cursors["todos:demo"], "number", "the subscription has a committed delivery position");
    const page = await post("pull", JSON.stringify({ cursors: { "todos:demo": beforePage.cursors["todos:demo"] }, models: { Todo: 1 } }));
    await client.client.applyPull(page);
    const afterPage = await client.syncState();
    assert.ok(afterPage.cursors["todos:demo"] > beforePage.cursors["todos:demo"], "page advances the cursor after the receipt");
    assert.equal((await client.models.todo.get({ id: "direct" }))?.title, "C", "the later page cannot regress receipt authority");
    assert.equal(result.todos[0]?.title, "A", "result does not change after later settlement");
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});

test("store selects which Search Query outputs update local Models on both routes", async () => {
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
    const unstored = await client.queries.searchTodos({ query: "storeq" }, { store: false });
    assert.deepEqual(unstored.todos.map((todo) => todo.title), ["storeq a", "storeq b"]);
    assert.equal(unstored.first?.title, "storeq a");
    assert.equal(await local("store-a"), null);
    assert.equal(await localStamp("store-a"), null);
    assert.equal(await serverStamps("store-a"), 0, "no output-only stamp allocation");
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(notifications, settled, "no Model notification for a disabled-only read");

    // Mixed: first is stored, a record only in todos is not.
    const mixed = await client.queries.searchTodos({ query: "storeq" }, { store: { todos: false } });
    assert.equal(mixed.todos.length, 2);
    assert.equal((await local("store-a"))?.title, "storeq a", "enabled overlapping output stores its record");
    assert.equal(await local("store-b"), null, "disabled-only record is not stored");
    const stampA = await localStamp("store-a");
    assert.ok(stampA);

    // A cached row stays unchanged when a disabled read returns newer content.
    // Another writer changes the record and advances its stamp.
    await fixture.pool.query("UPDATE action_e2e_todo SET title='storeq a2' WHERE id='store-a'");
    await fixture.pool.query("UPDATE axton_record SET stamp=stamp+1 WHERE model='Todo' AND identity_key LIKE '%store-a%'");
    const newer = await client.queries.searchTodos({ query: "storeq" }, { store: false });
    assert.equal(newer.first?.title, "storeq a2", "result is this invocation's Loader snapshot");
    assert.equal((await local("store-a"))?.title, "storeq a", "cached row unchanged");
    assert.equal(await localStamp("store-a"), stampA);

    // Durable store:false behaves the same, through the queue and receipt.
    const durable = await client.queries.enqueue.searchTodos({ query: "storeq" }, { store: false });
    const outcome = await durable.wait();
    assert.equal(outcome.error, null);
    assert.equal(outcome.result!.todos[1]?.title, "storeq b");
    assert.equal(await local("store-b"), null);
    assert.equal((await local("store-a"))?.title, "storeq a");
    // The default stores every eligible output.
    const stored = await client.queries.enqueue.searchTodos({ query: "storeq" });
    assert.equal((await stored.wait()).error, null);
    assert.equal((await local("store-a"))?.title, "storeq a2");
    assert.equal((await local("store-b"))?.title, "storeq b");

    // Required mutation reconciliation is never disabled.
    await client.connection!.pause();
    const edit = await client.mutations.updateTodo({ todo: { id: "store-a", title: "  edited  " } }, { store: false });
    assert.equal((await local("store-a"))?.title, "  edited  ", "optimistic edit");
    const snapshot = await client.queries.searchTodos({ query: "storeq" }, { store: false });
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
    client = await GeneratedClient.open({ path, server: server() });
    // One session establishes the subscription's origin, then the socket is
    // paused so the Action is pushed by hand: the page below is pulled from that
    // committed cursor, never from zero (#150).
    const subscription = await client.scopes.subscribe("todos:demo");
    await wait(async () => subscription.status.initialization === "ready", "first initialization");
    await client.connection!.pause();
    const origin = (await client.syncState()).cursors["todos:demo"];
    assert.equal(typeof origin, "number");
    await client.mutations.addTodo({ todo: { id: "replay", title: "  saved  " } });
    const frozen = await client.client.freeze();
    assert.ok(frozen);
    const first = await post("mutations", frozen);
    const page = await post("pull", JSON.stringify({ cursors: { "todos:demo": origin }, models: { Todo: 1 } }));
    await client.client.applyPull(page);
    const handled = fixture.handlerCalls;
    const loaded = fixture.loaderCalls;
    assert.equal((await client.models.todo.get({ id: "replay" }))?.title, "saved", "the page delivered the published record");
    assert.equal((await client.syncState()).pending, 1, "page authority alone does not complete the Action");
    await client.close();
    await fixture.pool.query("ALTER TABLE action_e2e_todo ADD COLUMN note text");
    let unexpected = 0;
    const forbidden = async (): Promise<never> => { unexpected++; throw Error("cached Action reexecuted"); };
    const mutations: EvolvedMutations<PgClient> = {
      addTodo: { v1: forbidden, v2: forbidden },
      updateTodo: { v1: forbidden, v2: forbidden },
      deleteTodo: forbidden,
      sendEmail: forbidden,
      searchTodos: forbidden,
      retitleTodos: { v1: forbidden, v2: forbidden },
    };
    // SearchTodos retains a Mutation v1 and Query v2/v3: each registers under its own kind.
    const queries: EvolvedQueries<PgClient> = { searchTodos: { v2: forbidden, v3: forbidden } };
    const loaders: EvolvedLoaders<PgClient> = { todo: { v1: forbidden, v2: forbidden } };
    const evolvedBackend = createEvolvedBackend<PgClient>({ database: pg(fixture.pool), authenticate: devAuth(), mutations, queries, loaders });
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

test("Queries read fresh on the direct route and at execution time when enqueued", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-query-fresh-"));
  const path = join(directory, "client.sqlite");
  let client: GeneratedClient | undefined;
  const queued = async () => (await client!.readSql("SELECT count(*) AS n FROM axton_mutation"))[0]!.n;
  try {
    await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('fresh-a','freshq one')");
    client = await GeneratedClient.open({ path, server: server() });
    const calls = fixture.queryCalls;
    let first: Awaited<ReturnType<typeof client.queries.searchTodos>> | undefined;
    let second: typeof first;
    const paths = await requestedPaths(async () => {
      first = await client!.queries.searchTodos({ query: "freshq" });
      await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('fresh-b','freshq two')");
      second = await client!.queries.searchTodos({ query: "freshq" });
    });
    assert.deepEqual(paths, ["/sync/actions", "/sync/actions"], "a default Query takes the direct path, never the queue");
    assert.deepEqual(first!.labels, ["fresh-a"]);
    assert.deepEqual(second!.labels, ["fresh-a", "fresh-b"], "each direct invocation reads again");
    assert.equal(await queued(), 0, "a direct Query writes no queue metadata");
    assert.equal((await client.syncState()).pending, 0);
    assert.equal(fixture.queryCalls, calls + 2);

    // Enqueued while paused: a durable intent that reads when it executes.
    await client.connection!.pause();
    const later = await client.queries.enqueue.searchTodos({ query: "freshq" });
    assert.equal(later.status, "pending");
    assert.equal(await queued(), 1);
    await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('fresh-c','freshq three')");
    await client.connection!.resume();
    const outcome = await later.wait();
    assert.equal(outcome.error, null);
    assert.deepEqual(outcome.result!.labels, ["fresh-a", "fresh-b", "fresh-c"], "read at execution, not at enqueue");

    // A queued Query survives reopen and derives no local optimism.
    await client.connection!.pause();
    await client.queries.enqueue.searchTodos({ query: "freshq" });
    await client.close();
    await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('fresh-d','freshq four')");
    client = await GeneratedClient.open({ path });
    assert.equal((await client.syncState()).pending, 1, "the queued Query survived SQLite reopen");
    const intents = await client.readSql("SELECT m.name, m.version, count(o.ordinal) AS operations FROM axton_mutation m LEFT JOIN axton_mutation_operation o ON o.ordinal = m.ordinal GROUP BY m.ordinal");
    assert.deepEqual(intents.map((row) => [row.name, row.version, row.operations]), [["SearchTodos", 2, 0]], "a queued Query derives no local Model operations");
    await client.connect(server());
    await wait(async () => (await client!.syncState()).pending === 0, "reopened queued Query settlement");
    assert.equal((await client.models.todo.get({ id: "fresh-d" }))?.title, "freshq four", "its default store:true outputs updated local Models");
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});

test("a direct Query retry with the same call ID replays its saved result", async () => {
  await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('replay-q','replayq one')");
  const call = (args: object) => JSON.stringify({ call: { callId: "01890f47-1234-7123-8123-1234567890aa", name: "SearchTodos", version: 2, args }, models: { Todo: 1 } });
  const first = await post("actions", call({ query: "replayq" }));
  assert.deepEqual(first.completion.outcome.result.labels, ["replay-q"]);
  const handled = fixture.handlerCalls;
  const loaded = fixture.loaderCalls;
  await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('replay-r','replayq two')");
  const retried = await post("actions", call({ query: "replayq" }));
  assert.deepEqual(retried, first, "the saved result is replayed, not a fresh read");
  assert.equal(fixture.handlerCalls, handled, "the Query handler did not run again");
  assert.equal(fixture.loaderCalls, loaded, "no Loader ran again");
  const conflict = await post("actions", call({ query: "other" }));
  assert.equal(conflict.completion.outcome.code, "call.identity_conflict");
  // The Query's retained kind comes from the backend: v1 is the Mutation contract.
  const retained = await post("actions", JSON.stringify({ call: { callId: "01890f47-1234-7123-8123-1234567890ab", name: "SearchTodos", version: 1, args: { query: "replayq" } }, models: { Todo: 1 } }));
  assert.equal(retained.completion.outcome.code, "search.v1_retired", "the retained Mutation v1 runs its own handler");
});

test("a direct Mutation resolves after the backend commits and its authority applies", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-mutation-direct-"));
  let client: GeneratedClient | undefined;
  try {
    client = await GeneratedClient.open({ path: join(directory, "client.sqlite"), server: server() });
    let done: Awaited<ReturnType<typeof client.mutations.call.addTodo>> | undefined;
    const paths = await requestedPaths(async () => { done = await client!.mutations.call.addTodo({ todo: { id: "direct-m", title: "  direct  " } }); });
    assert.deepEqual(paths, ["/sync/actions"], "a direct Mutation never enters the queue");
    assert.deepEqual(done, { todo: { id: "direct-m", title: "direct" } }, "the input-bound result is the committed Loader snapshot");
    assert.deepEqual((await fixture.pool.query("SELECT title FROM action_e2e_todo WHERE id='direct-m'")).rows, [{ title: "direct" }]);
    assert.equal((await client.models.todo.get({ id: "direct-m" }))?.title, "direct", "authority applied before resolving");
    assert.equal((await client.readSql("SELECT count(*) AS n FROM axton_mutation"))[0]!.n, 0, "no queue row and no optimism");
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});

test("store:false cannot suppress authority a Mutation's own changes require", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-mutation-store-"));
  let client: GeneratedClient | undefined;
  try {
    await fixture.pool.query("INSERT INTO action_e2e_todo(id,title) VALUES('retitle-a','retitleq a'),('retitle-b','retitleq b')");
    client = await GeneratedClient.open({ path: join(directory, "client.sqlite"), server: server() });
    const direct = await client.mutations.call.retitleTodos({ query: "retitleq", title: "retitleq direct" }, { store: false });
    assert.deepEqual(direct.todos.map((todo) => todo.title), ["retitleq direct", "retitleq direct"]);
    assert.equal((await client.models.todo.get({ id: "retitle-a" }))?.title, "retitleq direct", "handler-reported changes are required authority");
    assert.equal((await client.models.todo.get({ id: "retitle-b" }))?.title, "retitleq direct");
    const durable = await client.mutations.retitleTodos({ query: "retitleq", title: "retitleq durable" }, { store: { todos: false, first: false } });
    const outcome = await durable.wait();
    assert.equal(outcome.error, null);
    assert.equal(outcome.result!.first?.title, "retitleq durable");
    assert.equal((await client.models.todo.get({ id: "retitle-b" }))?.title, "retitleq durable", "the durable route applies the same required authority");
  } finally { await client?.close(); await rm(directory, { recursive: true, force: true }); }
});
