import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createClient } from "../../packages/client-js/runtime.mts";
import {
  Client,
  Transaction,
  type ActionCall,
  type ActionOutcome,
} from "../../packages/client-js/index.mts";
import { GeneratedClient } from "./client.ts";
import { makeActions } from "./generated.ts";

test("generated Action codecs retain null, lists, omitted patches and DateTime identity", async () => {
  const calls: {
    name: string;
    version: number;
    args: Record<string, unknown>;
  }[] = [];
  const actions = makeActions({
    async invokeAction(
      name: string,
      version: number,
      args: object,
      decode: (value: unknown) => unknown,
    ) {
      calls.push({ name, version, args: args as Record<string, unknown> });
      return {
        status: "succeeded" as const,
        async wait() {
          return { result: decode(null), error: null };
        },
      };
    },
    async invokeDirectAction(
      name: string,
      version: number,
      args: object,
      decode: (value: unknown) => unknown,
    ) {
      calls.push({ name, version, args: args as Record<string, unknown> });
      return decode(
        name === "RemoveMoment"
          ? { moment: { at: "2026-01-01T00:00:00.000Z" } }
          : name === "Put"
            ? {
                todo: {
                  id: "one",
                  title: "server",
                  at: "2026-01-01T00:00:00.000Z",
                  status: "closed",
                  note: null,
                },
                echoed: "2026-01-01T00:00:00.000Z",
                status: "closed",
              }
            : { todo: null, echoed: "2026-01-01T00:00:00.000Z" },
      );
    },
  });
  const at = new Date("2026-01-01T00:00:00.000Z");
  await actions.put({
    todo: { id: "one", title: "A", at, status: "open", note: null },
    when: at,
    statuses: ["open", "closed"],
    note: null,
  });
  assert.equal(calls[0]?.name, "Put");
  assert.equal(calls[0]?.version, 2);
  assert.deepEqual(calls[0]?.args, {
    todo: {
      id: "one",
      title: "A",
      at: at.toISOString(),
      status: "open",
      note: null,
    },
    when: at.toISOString(),
    statuses: ["open", "closed"],
    note: null,
  });
  await actions.change({ at });
  assert.deepEqual(calls[1]?.args, { todo: null, at: at.toISOString() });
  await actions.change({ todo: { id: "one", title: "B" }, at });
  assert.deepEqual(calls[2]?.args, {
    todo: { id: "one", title: "B" },
    at: at.toISOString(),
  });
  await actions.clear({ todo: [{ id: "one" }] });
  assert.deepEqual(calls[3]?.args, { todo: [{ id: "one" }] });
  await actions.mark({ moment: { at, title: "X" } });
  assert.deepEqual(calls[4]?.args, {
    moment: { at: at.toISOString(), title: "X" },
  });
  const result = await actions.call.change({ at });
  assert.ok(result.echoed instanceof Date);
  assert.equal(result.todo, null);
  const removed = await actions.call.removeMoment({ moment: { at } });
  assert.ok(removed.moment.at instanceof Date);
  assert.equal(removed.moment.at.toISOString(), at.toISOString());
  const put = await actions.call.put({
    todo: { id: "one", title: "A", at, status: "open", note: null },
    when: at,
    statuses: ["open"],
    note: null,
  });
  assert.equal(put.status, "closed");
  assert.ok(put.todo.at instanceof Date);
  assert.ok(put.echoed instanceof Date);
});

test("generated Actions resolve through the native pump and direct transport while Models stay local", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-generated-actions-"));
  const native = createRequire(import.meta.url)(
    "../../bindings/node/axton-node.node",
  );
  const row = {
    id: "one",
    title: "server",
    at: "2026-01-01T00:00:00.000Z",
    status: "open",
    note: null,
  };
  let pushes = 0;
  let directCalls = 0;
  const InjectedClient = createClient(native, Transaction, () => ({
    open() {},
    async push(kind: string, bodyText: string) {
      const body = JSON.parse(bodyText);
      if (kind === "action") {
        directCalls++;
        return JSON.stringify({
          completion: {
            callId: body.call.callId,
            outcome: { status: "succeeded", result: { todo: row } },
          },
          records: [],
        });
      }
      assert.equal(kind, "push");
      pushes++;
      return JSON.stringify({
        clientId: body.clientId,
        batchSequence: body.batchSequence,
        rejections: [],
        records: [],
        completions: body.mutations.map((mutation: { callId: string }) => ({
          callId: mutation.callId,
          outcome: { status: "succeeded", result: null },
        })),
      });
    },
  }));
  const originalOpen = Client.open;
  Client.open = ((options: Parameters<typeof Client.open>[0]) =>
    InjectedClient.open(options)) as typeof Client.open;
  let client: GeneratedClient | undefined;
  try {
    client = await GeneratedClient.open({
      path: join(directory, "state.sqlite"),
    });
    const local = { ...row, title: "local", at: new Date(row.at) };
    await client.models.todo.create(local);
    assert.equal((await client.models.todo.get({ id: "one" }))?.title, "local");
    await client.models.todo.update({ id: "one" }, { title: "local edited" });
    assert.equal(
      (await client.models.todo.get({ id: "one" }))?.title,
      "local edited",
    );
    assert.equal((await client.syncState()).pending, 0);
    const seen: string[] = [];
    const stop = client.models.todo.watch({}, (rows) =>
      seen.push(rows[0]?.title ?? "empty"),
    );
    await client.models.todo.delete({ id: "one" });
    await client.models.todo.create(local);
    await new Promise((resolve) => setTimeout(resolve, 15));
    stop();
    assert.ok(seen.includes("empty") && seen.includes("local"));
    assert.equal((await client.syncState()).pending, 0);

    const connection = await client.connect({
      url: "http://unused",
      token: "alice",
    });
    const call: ActionCall<void> = await client.actions.ping({});
    const settled: ActionOutcome<void> = await call.wait();
    assert.equal(settled.error, null);
    assert.equal(call.status, "succeeded");
    assert.deepEqual(await call.wait(), settled);
    assert.equal(pushes, 1);
    const found = await client.actions.call.find({ at: new Date(row.at) });
    assert.equal(found.todo?.title, "server");
    assert.ok(found.todo?.at instanceof Date);
    await connection.close();
    const pending = await client.actions.change({
      todo: { id: "one", title: "optimistic" },
      at: new Date(row.at),
    });
    assert.equal(pending.status, "pending");
    assert.equal(
      (await client.models.todo.get({ id: "one" }))?.title,
      "optimistic",
    );
    assert.equal(
      found.todo?.title,
      "server",
      "the returned Loader snapshot does not track local optimism",
    );
    assert.equal(directCalls, 1);
    assert.equal(pushes, 1);
    assert.equal((await client.syncState()).pending, 1);
    await client.close();
    assert.equal((await pending.wait()).error?.code, "client.closed");
  } finally {
    Client.open = originalOpen;
    await client?.close();
    await rm(directory, { recursive: true, force: true });
  }
});
