import test from "node:test";
import assert from "node:assert/strict";
import { startConnection } from "../../../packages/client-js/connection.mts";
test("wake arriving during idle decision cannot be lost", async () => {
  let release;
  const gate = new Promise((r) => (release = r));
  let calls = 0;
  const connection = await startConnection(
    async (event) => {
      if (event === "next") {
        if (++calls === 1) await gate;
        return { type: "idle" };
      }
    },
    async () => {},
    async () => "",
  );
  await connection.wake();
  release();
  await new Promise((r) => setTimeout(r, 10));
  assert.ok(calls >= 2);
  await connection.close();
});
test("close aborts an uncooperative network promise and prevents late completion", async () => {
  const events = [];
  let entered;
  const begun = new Promise((r) => (entered = r));
  const connection = await startConnection(
    async (event) => {
      events.push(event);
      return { type: "sync" };
    },
    async (request) => {
      entered();
      await request("push", "body");
    },
    async () => new Promise(() => {}),
  );
  await begun;
  await connection.close();
  await new Promise((r) => setImmediate(r));
  assert.ok(events.includes("stop"));
  assert.ok(!events.includes("success"));
  assert.ok(!events.includes("failure"));
});

test("closed connection controls cannot affect a replacement driver", async () => {
  const events = [];
  const connection = await startConnection(
    async (event) => {
      events.push(event);
      return { type: "idle" };
    },
    async () => {},
    async () => "",
  );
  await connection.close();
  const ended = events.length;
  await connection.pause();
  await connection.resume();
  await connection.wake();
  await connection.close();
  assert.equal(events.length, ended);
});
test("closing an old client connection twice preserves ownership of the replacement", async () => {
  const { Client } = await import("../../../packages/client-js/index.mts");
  const { mkdtemp, rm, readFile } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const directory = await mkdtemp(join(tmpdir(), "axton-connection-"));
  const schema = JSON.parse(
    await readFile(
      new URL("../../../fixtures/schemas/entry.json", import.meta.url),
      "utf8",
    ),
  );
  const client = await Client.open({
    path: join(directory, "client.sqlite"),
    schema,
  });
  let second;
  try {
    const first = await client.connect({url:"http://127.0.0.1:1",token:"secret"});
    await first.close();
    second = await client.connect({url:"http://127.0.0.1:1",token:"secret"});
    await first.close();
    await assert.rejects(
      client.connect({url:"http://127.0.0.1:1",token:"secret"}),
      /already active/,
    );
  } finally {
    await second?.close();
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});

test("client close waits for in-flight connection setup and remains idempotent", async () => {
  const { Client } = await import("../../../packages/client-js/index.mts");
  const { mkdtemp, rm, readFile } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const directory = await mkdtemp(join(tmpdir(), "axton-connection-close-"));
  const schema = JSON.parse(
    await readFile(
      new URL("../../../fixtures/schemas/entry.json", import.meta.url),
      "utf8",
    ),
  );
  const client = await Client.open({
    path: join(directory, "client.sqlite"),
    schema,
  });
  const errors = [];
  try {
    const starting = client.connect(
      {url:"http://127.0.0.1:1",token:"secret"},
      { onError: (error) => errors.push(error) },
    );
    await Promise.all([starting, client.close()]);
    await new Promise((r) => setImmediate(r));
    assert.deepEqual(errors, []);
    await client.close();
    await (await starting).close();
    await assert.rejects(
      client.connect({url:"http://127.0.0.1:1",token:"secret"}),
      /closed/,
    );
  } finally {
    await client.close().catch(() => {});
    await rm(directory, { recursive: true, force: true });
  }
});
