import test from "node:test";
import assert from "node:assert/strict";
import {
  ActionRegistry,
  ActionError,
} from "../../../packages/client-js/actions.mts";
import { Client } from "../../../packages/client-js/index.mts";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import { createClient } from "../../../packages/client-js/runtime.mts";
import { Transaction } from "../../../packages/client-js/transaction.mts";

test("observer retains a live wait, settles once, and caches the outcome", async () => {
  const registry = new ActionRegistry();
  const call = registry.register("one", (value) => ({ title: value.title }));
  assert.equal(call.status, "pending");
  const pending = call.wait();
  registry.complete({
    callId: "one",
    outcome: { status: "succeeded", result: { title: "A" } },
  });
  const first = await pending;
  assert.deepEqual(first, { result: { title: "A" }, error: null });
  assert.equal(call.status, "succeeded");
  assert.strictEqual(await call.wait(), first);
  assert.equal(registry.activeCount, 0);
});

test("observer converts backend failures and decoder failures into terminal outcomes", async () => {
  const registry = new ActionRegistry();
  const rejected = registry.register("rejected", (value) => value);
  const malformed = registry.register("malformed", () => {
    throw Error("bad result");
  });
  registry.complete({
    callId: "rejected",
    outcome: { status: "failed", code: "denied", execution: "rejected" },
  });
  registry.complete({
    callId: "malformed",
    outcome: { status: "succeeded", result: {} },
  });
  assert.equal((await rejected.wait()).error.code, "denied");
  assert.equal(
    (await malformed.wait()).error.code,
    "action.observation_failed",
  );
  assert.equal(malformed.status, "failed");
});

test("decoded results remain snapshots after the source object changes", async () => {
  const registry = new ActionRegistry();
  const call = registry.register("snapshot", (value) => ({
    title: value.title,
  }));
  const source = { title: "A" };
  registry.complete({
    callId: "snapshot",
    outcome: { status: "succeeded", result: source },
  });
  source.title = "B";
  assert.equal((await call.wait()).result.title, "A");
});

test("closing settles held handles even before wait is called", async () => {
  const registry = new ActionRegistry();
  const call = registry.register("one", (value) => value);
  registry.close();
  assert.equal(call.status, "failed");
  assert.equal((await call.wait()).error.code, "client.closed");
});

test("weak routing sweeps dead states but active waits remain live", async () => {
  const references = [];
  const registry = new ActionRegistry((state) => {
    const reference = {
      state,
      deref() {
        return this.state;
      },
    };
    references.push(reference);
    return reference;
  });
  registry.register("abandoned", (value) => value);
  references[0].state = undefined;
  registry.register("other", (value) => value);
  assert.equal(registry.routingCount, 1);
  let live = registry.register("live", (value) => value);
  const pending = live.wait();
  live = null;
  references[2].state = undefined;
  registry.complete({
    callId: "live",
    outcome: { status: "succeeded", result: 3 },
  });
  assert.equal((await pending).result, 3);
  assert.equal(registry.activeCount, 0);
});

test("missing WeakRef rejects before registration", () => {
  const registry = new ActionRegistry(null);
  assert.throws(
    () => registry.assertSupported(),
    (error) =>
      error instanceof ActionError &&
      error.code === "action.unsupported_runtime",
  );
  assert.equal(registry.routingCount, 0);
});

async function withClient(body) {
  const directory = await mkdtemp(join(tmpdir(), "axton-action-observer-"));
  const schema = JSON.parse(
    await readFile(
      new URL("../../../fixtures/schemas/entry.json", import.meta.url),
      "utf8",
    ),
  );
  schema.actions = [{ name: "Ping", version: 1, inputs: [], outputs: [] }];
  const client = await Client.open({ path: join(directory, "db"), schema });
  try {
    await body(client);
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
}

test("durable invocation registers before wake and drop settles its handle", async () => {
  await withClient(async (client) => {
    const call = await client.invokeAction("Ping", 1, {}, () => undefined);
    assert.equal(call.status, "pending");
    const waiting = call.wait();
    const tasks = await client.syncState();
    assert.equal(tasks.pending, 1);
    await client.drop((await client.pendingTasks())[0]?.ordinal ?? 1);
    const outcome = await waiting;
    assert.equal(outcome.error.code, "dropped");
    assert.equal(call.status, "failed");
    assert.equal((await client.syncState()).pending, 0);
  });
});

test("close fails a durable handle before wait is called", async () => {
  await withClient(async (client) => {
    const call = await client.invokeAction("Ping", 1, {}, () => undefined);
    await client.close();
    assert.equal((await call.wait()).error.code, "client.closed");
  });
});

test("standalone local writes stay out of the durable queue and reach reads and watch", async () => {
  await withClient(async (client) => {
    const observed = [];
    const stop = client.watch("Entry", {}, (rows) => observed.push(rows));
    try {
      await client.direct({
        model: "Entry",
        op: "create",
        identity: { id: "one" },
        values: { text: "A" },
      });
      assert.equal((await client.read("Entry", { id: "one" })).text, "A");
      assert.equal((await client.syncState()).pending, 0);
      await new Promise((resolve) => setImmediate(resolve));
      assert.ok(observed.some((rows) => rows.some((row) => row.text === "A")));
    } finally {
      stop();
    }
  });
});

test("the real native pump settles a waiting handle before a throwing diagnostic listener", async () => {
  const native = createRequire(import.meta.url)(
    "../../../bindings/node/axton-node.node",
  );
  const directory = await mkdtemp(join(tmpdir(), "axton-action-pump-"));
  const schema = {
    enums: [],
    models: [],
    actions: [{ name: "Ping", version: 1, inputs: [], outputs: [] }],
  };
  let requests = 0;
  const PumpClient = createClient(native, Transaction, () => ({
    open() {},
    push: async (kind, bodyText) => {
      const body = JSON.parse(bodyText);
      if (kind !== "push") throw Error(`unexpected ${kind}`);
      requests++;
      return JSON.stringify({
        clientId: body.clientId,
        batchSequence: body.batchSequence,
        rejections: [],
        records: [],
        completions: body.mutations.map((mutation) => ({
          callId: mutation.callId,
          outcome: { status: "succeeded", result: null },
        })),
      });
    },
  }));
  const client = await PumpClient.open({ path: join(directory, "db"), schema });
  try {
    const connection = await client.connect({
      url: "http://unused",
      token: "token",
    });
    client.onActionCompletion(() => {
      throw Error("diagnostic failed");
    });
    const call = await client.invokeAction("Ping", 1, {}, () => undefined);
    const outcome = await Promise.race([
      call.wait(),
      new Promise((_, reject) =>
        setTimeout(() => reject(Error("wait timed out")), 1000),
      ),
    ]);
    assert.deepEqual(outcome, { result: undefined, error: null });
    assert.equal(call.status, "succeeded");
    assert.equal(requests, 1);
    assert.equal((await client.syncState()).pending, 0);
    await connection.close();
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});

test("direct invocation decodes a committed response and maps unavailable transport errors", async () => {
  const native = createRequire(import.meta.url)(
    "../../../bindings/node/axton-node.node",
  );
  const directory = await mkdtemp(join(tmpdir(), "axton-action-direct-"));
  const schema = {
    enums: [],
    models: [],
    actions: [{ name: "Ping", version: 1, inputs: [], outputs: [] }],
  };
  const DirectClient = createClient(native, Transaction, () => ({
    open() {},
    push: async (kind, bodyText) => {
      if (kind !== "action") throw Error(`unexpected ${kind}`);
      const body = JSON.parse(bodyText);
      return JSON.stringify({
        completion: {
          callId: body.call.callId,
          outcome: { status: "succeeded", result: null },
        },
        records: [],
      });
    },
  }));
  const client = await DirectClient.open({
    path: join(directory, "db"),
    schema,
  });
  try {
    await assert.rejects(
      client.invokeDirectAction("Ping", 1, {}, () => undefined),
      (error) =>
        error instanceof ActionError && error.code === "action.unavailable",
    );
    const connection = await client.connect({
      url: "http://unused",
      token: "token",
    });
    assert.equal(
      await client.invokeDirectAction("Ping", 1, {}, () => undefined),
      undefined,
    );
    assert.equal((await client.syncState()).pending, 0);
    await connection.close();
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});

test("unsupported runtime fails before durable submission", async () => {
  await withClient(async (client) => {
    const original = globalThis.WeakRef;
    try {
      globalThis.WeakRef = undefined;
      await assert.rejects(
        client.invokeAction("Ping", 1, {}, () => undefined),
        (error) => error.code === "action.unsupported_runtime",
      );
    } finally {
      globalThis.WeakRef = original;
    }
    assert.equal((await client.syncState()).pending, 0);
  });
});

test("standalone direct writes and Actions reject from an active transaction callback", async () => {
  await withClient(async (client) => {
    await client.transaction(async () => {
      await assert.rejects(
        client.direct({
          model: "Entry",
          op: "create",
          identity: { id: "blocked" },
          values: { text: "A" },
        }),
        /transaction_active/,
      );
      await assert.rejects(
        client.invokeAction("Ping", 1, {}, () => undefined),
        (error) =>
          error instanceof ActionError && error.code === "transaction_active",
      );
      await assert.rejects(
        client.invokeDirectAction("Ping", 1, {}, () => undefined),
        (error) =>
          error instanceof ActionError && error.code === "transaction_active",
      );
    });
    assert.equal((await client.syncState()).pending, 0);
  });
});

test("close racing a committed submit still yields a terminal handle", async () => {
  const binding = createRequire(import.meta.url)(
    "../../../bindings/node/axton-node.node",
  );
  const directory = await mkdtemp(join(tmpdir(), "axton-action-close-race-"));
  const schema = {
    enums: [],
    models: [],
    actions: [{ name: "Ping", version: 1, inputs: [], outputs: [] }],
  };
  let entered;
  let release;
  const submitted = new Promise((resolve) => {
    entered = resolve;
  });
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const native = {
    async clientCall(request) {
      const result = await binding.clientCall(request);
      if (JSON.parse(request).op === "submitAction") {
        entered();
        await gate;
      }
      return result;
    },
  };
  const RaceClient = createClient(native, Transaction, () => {
    throw Error("network not configured");
  });
  const client = await RaceClient.open({ path: join(directory, "db"), schema });
  try {
    const callPromise = client.invokeAction("Ping", 1, {}, () => undefined);
    await submitted;
    const closing = client.close();
    release();
    const call = await callPromise;
    assert.equal((await call.wait()).error.code, "client.closed");
    await closing;
  } finally {
    release();
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});
