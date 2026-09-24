import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createClient } from "../../../packages/client-js/runtime.mts";
import { Transaction } from "../../../packages/client-react-native/transaction.mts";

test("mobile durable diagnostics do not stop later native Actions", async () => {
  const native = createRequire(import.meta.url)(
    "../../../bindings/node/axton-node.node",
  );
  const directory = await mkdtemp(join(tmpdir(), "axton-rn-reports-"));
  const schema = JSON.parse(
    await readFile(
      new URL("../../../fixtures/schemas/entry.json", import.meta.url),
      "utf8",
    ),
  );
  schema.actions = [{ name: "Ping", version: 1, inputs: [], outputs: [] }];
  let requests = 0;
  const Client = createClient(native, Transaction, () => ({
    open() {},
    push: async (kind, bodyText) => {
      assert.equal(kind, "push");
      const body = JSON.parse(bodyText);
      requests++;
      return JSON.stringify({
        clientId: body.clientId,
        batchSequence: body.batchSequence,
        rejections: [],
        completions: body.mutations.map((mutation) => ({
          callId: mutation.callId,
          outcome: { status: "succeeded", result: null },
        })),
        records:
          requests === 3
            ? []
            : ["a", "b"].map((id) => ({
                model: "Entry",
                identity: { id },
                stamp: 1,
                state: {
                  text: requests === 1 ? "first" : "second",
                  note: null,
                },
              })),
      });
    },
  }));
  const client = await Client.open({ path: join(directory, "db"), schema });
  const previous = globalThis.reportError;
  const diagnostic = Error("mobile diagnostic failed");
  const observed = [];
  const reports = [];
  globalThis.reportError = (error) => observed.push(error);
  try {
    const connection = await client.connect(
      { url: "http://unused", token: "token" },
      {
        onError: (report) => {
          reports.push(report);
          throw diagnostic;
        },
      },
    );
    for (let i = 0; i < 3; i++) {
      const call = await client.invokeAction("Ping", 1, {}, () => undefined);
      const outcome = await Promise.race([
        call.wait(),
        new Promise((_, reject) =>
          setTimeout(() => reject(Error("pump stalled")), 1000),
        ),
      ]);
      assert.equal(outcome.error, null);
    }
    assert.equal(requests, 3);
    assert.equal(reports.length, 2);
    assert.ok(reports.every((report) => report.message.includes("conflict")));
    assert.deepEqual(observed, [diagnostic, diagnostic]);
    assert.equal((await client.syncState()).pending, 0);
    await connection.close();
  } finally {
    globalThis.reportError = previous;
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});
import {
  makeActions,
  liveModels,
  schema as generatedSchema,
} from "../../action-runtime-ts/generated.ts";

test("mobile host runtime settles Actions and standalone local writes", async () => {
  const native = createRequire(import.meta.url)(
    "../../../bindings/node/axton-node.node",
  );
  const Client = createClient(native, Transaction, () => {
    throw Error("network not configured");
  });
  const directory = await mkdtemp(join(tmpdir(), "axton-rn-actions-"));
  const schema = {
    enums: [],
    models: [
      {
        name: "Entry",
        identity: ["id"],
        fields: [
          {
            name: "id",
            type: { kind: "scalar", name: "string" },
            nullable: false,
          },
          {
            name: "text",
            type: { kind: "scalar", name: "string" },
            nullable: false,
          },
        ],
      },
    ],
    actions: [{ name: "Ping", version: 1, inputs: [], outputs: [] }],
  };
  const client = await Client.open({ path: join(directory, "db"), schema });
  try {
    await client.direct({
      model: "Entry",
      op: "create",
      identity: { id: "one" },
      values: { text: "local" },
    });
    assert.equal((await client.read("Entry", { id: "one" })).text, "local");
    assert.equal((await client.syncState()).pending, 0);
    const call = await client.invokeAction("Ping", 1, {}, () => undefined);
    const waiting = call.wait();
    await client.drop(1);
    assert.equal((await waiting).error.code, "dropped");
    assert.equal(call.status, "failed");
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});

test("generated Action and Model bindings run through the mobile host adapter", async () => {
  const native = createRequire(import.meta.url)(
    "../../../bindings/node/axton-node.node",
  );
  const Client = createClient(native, Transaction, () => {
    throw Error("network not configured");
  });
  const directory = await mkdtemp(
    join(tmpdir(), "axton-rn-generated-actions-"),
  );
  const client = await Client.open({
    path: join(directory, "db"),
    schema: generatedSchema,
  });
  try {
    const models = liveModels(client);
    await models.todo.create({
      id: "one",
      title: "local",
      at: new Date("2026-01-01T00:00:00Z"),
      status: "open",
      note: null,
    });
    assert.equal((await models.todo.get({ id: "one" }))?.title, "local");
    assert.equal((await client.syncState()).pending, 0);
    const call = await makeActions(client).ping({});
    await client.drop(1);
    assert.equal((await call.wait()).error?.code, "dropped");
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});
