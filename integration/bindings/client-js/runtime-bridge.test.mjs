import test from "node:test";
import assert from "node:assert/strict";
import { AsyncLocalStorage } from "node:async_hooks";
import { createRequire } from "node:module";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Bridge } from "../../../packages/client-js/bridge.mts";

// The SDK Bridge over the Rust-owned client runtime (#134): request routing,
// callback transactions, wake/drain dispatch and lifecycle, on the real
// Node carrier.
const native = createRequire(import.meta.url)(
  "../../../bindings/node/axton-node.node",
);
const schemaUrl = new URL(
  "../../../fixtures/schemas/entry.json",
  import.meta.url,
);
const schema = JSON.parse(await readFile(schemaUrl, "utf8"));
const envelopes = JSON.parse(
  await readFile(
    new URL("../../../fixtures/bridge/envelopes.json", import.meta.url),
    "utf8",
  ),
);
const key = (id) => ({ model: "Entry", identity: { id } });
const create = (id, text) => ({
  kind: "direct",
  operation: {
    model: "Entry",
    op: "create",
    identity: { id },
    values: { text },
  },
});
const update = (id, text) => ({
  kind: "direct",
  operation: {
    model: "Entry",
    op: "update",
    identity: { id },
    values: { text },
  },
});
const read = (id) => ({ kind: "read", key: key(id) });
const deferred = () => {
  let resolve;
  const promise = new Promise((r) => (resolve = r));
  return { promise, resolve };
};
/** Whether `promise` is still pending after the actor had time to run it. */
async function pending(promise, ms = 50) {
  let timer;
  const marker = Symbol();
  try {
    return (
      (await Promise.race([
        promise.then(
          () => undefined,
          () => undefined,
        ),
        new Promise(
          (resolve) => (timer = setTimeout(() => resolve(marker), ms)),
        ),
      ])) === marker
    );
  } finally {
    clearTimeout(timer);
  }
}
async function withBridge(body, carrier = native) {
  const directory = await mkdtemp(join(tmpdir(), "axton-bridge-"));
  const { bridge, opened } = await Bridge.open(carrier, {
    path: join(directory, "client.sqlite"),
    schema,
  });
  try {
    await body(bridge, opened);
  } finally {
    await bridge.close();
    await rm(directory, { recursive: true, force: true });
  }
}

test("open answers the client id and schema state", async () => {
  await withBridge(async (bridge, opened) => {
    assert.equal(typeof opened.clientId, "string");
    assert.deepEqual(opened.schema, {
      rebuilt: false,
      pending: null,
      lastRebuild: null,
    });
    assert.equal(bridge.closed, false);
  });
});

test("overlapping tasks return to the correct waiter while a callback owns the transaction", async () => {
  await withBridge(async (bridge) => {
    await bridge.task(create("e", "A"));
    const order = [];
    const entered = deferred();
    const inside = deferred();
    const gate = deferred();
    const transaction = bridge.transaction(async (transactionId) => {
      entered.resolve(transactionId);
      await bridge.transactionCommand(
        transactionId,
        undefined,
        update("e", "B"),
      );
      order.push("tx write");
      const row = await bridge.transactionCommand(
        transactionId,
        undefined,
        read("e"),
      );
      order.push(`tx read ${row.text}`);
      inside.resolve();
      await gate.promise;
    });
    void transaction.then(() => order.push("transaction"));
    const transactionId = await entered.promise;
    assert.equal(typeof transactionId, "string");
    const outside = bridge.task(read("e")).then((row) => {
      order.push(`outside read ${row.text}`);
      return row;
    });
    const status = bridge.task({ kind: "status" }).then((state) => {
      order.push("status");
      return state;
    });
    await inside.promise;
    assert.equal(await pending(outside), true, "ordinary read waits");
    assert.equal(await pending(status), true, "status waits");
    assert.deepEqual(order, ["tx write", "tx read B"]);
    gate.resolve();
    await transaction;
    assert.equal((await outside).text, "B");
    assert.equal(typeof (await status).clientId, "string");
    assert.deepEqual(order, [
      "tx write",
      "tx read B",
      "transaction",
      "outside read B",
      "status",
    ]);
  });
});

test("a command in the wrong scope fails without joining and rolls the unit back", async () => {
  await withBridge(async (bridge) => {
    let refused;
    await assert.rejects(
      bridge.transaction(async (transactionId) => {
        await bridge.transactionCommand(
          transactionId,
          undefined,
          create("w", "A"),
        );
        refused = await bridge
          .transactionCommand(transactionId, "sp999", read("w"))
          .catch((error) => error);
      }),
      /invalid transaction scope/,
    );
    assert.match(refused.message, /invalid transaction scope/);
    assert.equal(await bridge.task(read("w")), null);
  });
});

test("savepoint scopes are issued by the runtime and carried by nested commands", async () => {
  await withBridge(async (bridge) => {
    await bridge.transaction(async (transactionId) => {
      const { scope } = await bridge.transactionCommand(
        transactionId,
        undefined,
        {
          kind: "savepoint",
        },
      );
      assert.equal(typeof scope, "string");
      await bridge.transactionCommand(transactionId, scope, create("s", "A"));
      await bridge.transactionCommand(transactionId, scope, {
        kind: "rollbackSavepoint",
        scope,
      });
      await bridge.transactionCommand(
        transactionId,
        undefined,
        create("t", "B"),
      );
    });
    assert.equal(await bridge.task(read("s")), null);
    assert.equal((await bridge.task(read("t"))).text, "B");
  });
});

test("admission failure rejects its waiter and leaves the bridge usable; after close tasks reject client_closed", async () => {
  let refuse = true;
  const carrier = {
    runtimeOpen: (request, wake) => native.runtimeOpen(request, wake),
    runtimeSubmit(runtimeId, message) {
      if (refuse && JSON.parse(message).command?.kind === "status") {
        refuse = false;
        throw Error("client_closed");
      }
      native.runtimeSubmit(runtimeId, message);
    },
    runtimeDrain: (runtimeId) => native.runtimeDrain(runtimeId),
    runtimeDetach: (runtimeId) => native.runtimeDetach(runtimeId),
  };
  let closedBridge;
  await withBridge(async (bridge) => {
    closedBridge = bridge;
    await assert.rejects(bridge.task({ kind: "status" }), /client_closed/);
    assert.equal(
      typeof (await bridge.task({ kind: "status" })).clientId,
      "string",
    );
  }, carrier);
  assert.equal(closedBridge.closed, true);
  await assert.rejects(closedBridge.task({ kind: "status" }), /client_closed/);
  await closedBridge.close();
});

test("a throwing callback rejects with the same value and rolls back", async () => {
  await withBridge(async (bridge) => {
    const thrown = Error("abort");
    await assert.rejects(
      bridge.transaction(async (transactionId) => {
        await bridge.transactionCommand(
          transactionId,
          undefined,
          create("rolled", "A"),
        );
        throw thrown;
      }),
      (error) => error === thrown,
    );
    assert.equal(await bridge.task(read("rolled")), null);
    const odd = { reason: "not an Error" };
    await assert.rejects(
      bridge.transaction(async () => {
        throw odd;
      }),
      (error) => error === odd,
    );
  });
});

test("the callback runs in the async context of the transaction's caller", async () => {
  await withBridge(async (bridge) => {
    const storage = new AsyncLocalStorage();
    let seen;
    await storage.run("caller", () =>
      bridge.transaction(async () => {
        seen = storage.getStore();
      }),
    );
    assert.equal(seen, "caller");
  });
});

test("a thrown value without a usable message still rolls back and rejects with it", async () => {
  await withBridge(async (bridge) => {
    const opaque = Object.create(null);
    await assert.rejects(
      bridge.transaction(async (transactionId) => {
        await bridge.transactionCommand(
          transactionId,
          undefined,
          create("o", "A"),
        );
        throw opaque;
      }),
      (error) => error === opaque,
    );
    assert.equal(await bridge.task(read("o")), null);
  });
});

test("a caught command failure rejects at commit with the engine message", async () => {
  await withBridge(async (bridge) => {
    let caught;
    await assert.rejects(
      bridge.transaction(async (transactionId) => {
        await bridge.transactionCommand(
          transactionId,
          undefined,
          create("kept", "A"),
        );
        await bridge
          .transactionCommand(transactionId, undefined, {
            kind: "direct",
            operation: {
              model: "Missing",
              op: "create",
              identity: { id: "m" },
              values: {},
            },
          })
          .catch((error) => {
            caught = error;
          });
      }),
      (error) =>
        error instanceof Error &&
        error !== caught &&
        error.message === caught.message,
    );
    assert.ok(caught instanceof Error);
    assert.equal(await bridge.task(read("kept")), null);
  });
});

test("a failed open rejects with the engine message and detaches its runtime", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-bridge-open-"));
  const opened = [];
  const detached = [];
  const carrier = {
    runtimeOpen(request, wake) {
      const id = native.runtimeOpen(request, wake);
      opened.push(id);
      return id;
    },
    runtimeSubmit: (runtimeId, message) =>
      native.runtimeSubmit(runtimeId, message),
    runtimeDrain: (runtimeId) => native.runtimeDrain(runtimeId),
    runtimeDetach(runtimeId) {
      detached.push(runtimeId);
      native.runtimeDetach(runtimeId);
    },
  };
  try {
    const missing = join(directory, "missing", "deeper", "client.sqlite");
    await assert.rejects(
      Bridge.open(carrier, { path: missing, schema }),
      (error) =>
        error instanceof Error &&
        error.message.length > 0 &&
        error.message !== "client_closed",
    );
    assert.equal(opened.length, 1);
    assert.deepEqual(detached, opened);
    assert.throws(
      () => native.runtimeSubmit(opened[0], JSON.stringify({ type: "close" })),
      /client_closed/,
    );
    const { bridge } = await Bridge.open(carrier, {
      path: join(directory, "client.sqlite"),
      schema,
    });
    assert.equal(await bridge.task(read("none")), null);
    await bridge.close();
    assert.deepEqual(detached, opened);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("close during an open callback rolls back and settles every waiter", async () => {
  await withBridge(async (bridge) => {
    const entered = deferred();
    const gate = deferred();
    let late;
    const transaction = bridge.transaction(async (transactionId) => {
      await bridge.transactionCommand(
        transactionId,
        undefined,
        create("c", "A"),
      );
      entered.resolve(transactionId);
      await gate.promise;
      late = await bridge
        .transactionCommand(transactionId, undefined, read("c"))
        .catch((error) => error);
    });
    await entered.promise;
    const queued = bridge.task(read("c"));
    const closing = bridge.close();
    assert.strictEqual(bridge.close(), closing);
    await assert.rejects(transaction, /client_closed/);
    await assert.rejects(queued, /client_closed/);
    gate.resolve();
    await closing;
    await new Promise((resolve) => setImmediate(resolve));
    assert.match(late.message, /transaction_closed|client_closed/);
    assert.equal(bridge.closed, true);
  });
});

test("the shared envelope fixtures carry the shapes the bridge sends and switches on", () => {
  const inputTypes = new Set(envelopes.inputs.map((input) => input.type));
  assert.deepEqual([...inputTypes].sort(), [
    "callbackResult",
    "close",
    "effectResult",
    "task",
    "transactionCommand",
  ]);
  for (const input of envelopes.inputs) {
    switch (input.type) {
      case "task":
        assert.equal(typeof input.requestId, "string");
        assert.equal(typeof input.command.kind, "string");
        break;
      case "transactionCommand":
        assert.equal(typeof input.requestId, "string");
        assert.equal(typeof input.transactionId, "string");
        assert.ok(input.scope === undefined || typeof input.scope === "string");
        assert.equal(typeof input.command.kind, "string");
        break;
      case "callbackResult":
        assert.equal(typeof input.effectId, "string");
        assert.equal(typeof input.transactionId, "string");
        assert.equal(typeof input.ok, "boolean");
        assert.ok(
          input.ok ? !("error" in input) : typeof input.error === "string",
        );
        break;
      case "effectResult":
        assert.equal(typeof input.effectId, "string");
        assert.equal(typeof input.outcome.ok, "boolean");
        if (!input.outcome.ok) {
          assert.equal(typeof input.outcome.error.message, "string");
          assert.ok(
            input.outcome.error.status === undefined ||
              typeof input.outcome.error.status === "number",
          );
        }
        break;
      case "close":
        assert.deepEqual(Object.keys(input), ["type"]);
        break;
    }
  }
  const eventTypes = new Set(envelopes.events.map((event) => event.type));
  assert.deepEqual([...eventTypes].sort(), [
    "callCompleted",
    "cancelEffect",
    "changed",
    "effect",
    "laneSignal",
    "observerChanged",
    "report",
    "runtimeClosed",
    "taskCompleted",
  ]);
  const operations = new Set();
  const lanes = new Set();
  for (const event of envelopes.events) {
    switch (event.type) {
      case "taskCompleted":
        assert.equal(typeof event.requestId, "string");
        assert.equal(typeof event.ok, "boolean");
        assert.ok("value" in event);
        assert.ok(
          event.ok ? !("error" in event) : typeof event.error === "string",
        );
        break;
      case "effect":
        assert.equal(typeof event.effectId, "string");
        assert.equal(typeof event.operation.kind, "string");
        operations.add(event.operation.kind);
        if (event.operation.kind === "callback") {
          assert.equal(typeof event.operation.transactionId, "string");
          assert.equal(typeof event.operation.requestId, "string");
        }
        break;
      case "cancelEffect":
        assert.equal(typeof event.effectId, "string");
        break;
      case "callCompleted":
        assert.equal(typeof event.callId, "string");
        assert.equal(typeof event.outcome, "object");
        break;
      case "observerChanged":
        assert.equal(typeof event.observerId, "string");
        assert.ok("snapshot" in event);
        break;
      case "report":
        assert.ok(
          ["records", "error", "protocol"].includes(event.diagnostic.kind),
        );
        break;
      case "changed":
        assert.ok(event.tables.every((table) => typeof table === "string"));
        break;
      case "laneSignal":
        assert.equal(typeof event.signal.lane, "string");
        lanes.add(event.signal.lane);
        break;
      case "runtimeClosed":
        assert.deepEqual(Object.keys(event), ["type"]);
        break;
    }
  }
  assert.deepEqual([...operations].sort(), [
    "callback",
    "http",
    "prerequisite",
    "refreshAuth",
    "socket",
    "timer",
  ]);
  // Every DownlinkSignal the subscription projection understands.
  assert.deepEqual([...lanes].sort(), [
    "acknowledged",
    "bootstrap",
    "changed",
    "ended",
    "opened",
    "paused",
    "requests",
    "resumed",
    "stopped",
  ]);
});

test("the bridge dispatches every fixture event and answers effects it has no handler for", async () => {
  // A carrier that delivers the shared fixture events after a successful
  // open: the bridge must route each one, never stop on one, and answer an
  // effect nobody handles.
  let wake;
  let batches = [];
  const submitted = [];
  const detached = [];
  const carrier = {
    runtimeOpen(request, wakeRuntime) {
      const { requestId } = JSON.parse(request);
      wake = wakeRuntime;
      batches.push([
        {
          type: "taskCompleted",
          requestId,
          ok: true,
          value: {
            clientId: "c",
            schema: { rebuilt: false, pending: null, lastRebuild: null },
          },
        },
      ]);
      setImmediate(() => wake("9"));
      return "9";
    },
    runtimeSubmit(runtimeId, message) {
      assert.equal(runtimeId, "9");
      submitted.push(JSON.parse(message));
    },
    runtimeDrain(runtimeId) {
      assert.equal(runtimeId, "9");
      return JSON.stringify(batches.shift() ?? []);
    },
    runtimeDetach(runtimeId) {
      detached.push(runtimeId);
    },
  };
  const { bridge, opened } = await Bridge.open(carrier, {
    path: "unused",
    schema,
  });
  assert.equal(opened.clientId, "c");
  const seen = [];
  for (const type of [
    "callCompleted",
    "observerChanged",
    "report",
    "changed",
    "laneSignal",
  ])
    bridge.on(type, (event) => seen.push(event.type));
  const handled = [];
  bridge.onEffect("timer", (effectId, operation) =>
    handled.push([effectId, operation.millis]),
  );
  // Split the fixture events over two drains to exercise the recheck.
  const events = envelopes.events.filter(
    (event) => event.type !== "runtimeClosed",
  );
  batches = [events.slice(0, 5), events.slice(5)];
  wake("9");
  assert.deepEqual(seen, [
    "callCompleted",
    "observerChanged",
    "report",
    "report",
    "report",
    "changed",
    ...Array(9).fill("laneSignal"),
  ]);
  assert.deepEqual(handled, [["7", 250]]);
  const answered = submitted.filter((input) => input.type !== "callbackResult");
  assert.deepEqual(answered.map((input) => input.effectId).sort(), [
    "10",
    "11",
    "12",
    "6",
    "8",
    "9",
  ]);
  for (const input of answered)
    assert.deepEqual(input.outcome, {
      ok: false,
      error: { message: "unsupported effect" },
    });
  // A callback effect for a request the bridge does not route is refused.
  assert.deepEqual(
    submitted.filter((input) => input.type === "callbackResult"),
    [
      {
        type: "callbackResult",
        effectId: "5",
        transactionId: "tx7",
        ok: false,
        error: "unknown transaction",
      },
    ],
  );
  const task = bridge.task({ kind: "status" });
  const closing = bridge.close();
  assert.deepEqual(submitted.at(-1), { type: "close" });
  batches = [[{ type: "runtimeClosed" }]];
  wake("9");
  await assert.rejects(task, /client_closed/);
  await closing;
  assert.deepEqual(detached, ["9"]);
  assert.equal(bridge.closed, true);
});

test("a throwing listener does not stop the completion in the same drain batch", async () => {
  const original = globalThis.reportError;
  const reported = [];
  globalThis.reportError = (error) => reported.push(error);
  try {
    await withBridge(async (bridge) => {
      const failure = Error("listener failed");
      const seen = [];
      bridge.on("changed", () => {
        throw failure;
      });
      bridge.on("changed", (event) => seen.push(event.tables));
      assert.equal(await bridge.task(create("l", "A")), null);
      assert.deepEqual(reported, [failure]);
      assert.equal(seen.length, 1);
      assert.ok(seen[0].includes("Entry"));
    });
  } finally {
    if (original === undefined) delete globalThis.reportError;
    else globalThis.reportError = original;
  }
});

/** Run a module script and answer its exit code and output. */
function script(source) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      ["--input-type=module", "-e", source],
      { timeout: 20000 },
      (error, stdout, stderr) =>
        resolve({
          code: error ? (error.code ?? error.signal) : 0,
          stdout,
          stderr,
        }),
    );
  });
}
const bridgeModule = fileURLToPath(
  new URL("../../../packages/client-js/bridge.mts", import.meta.url),
);
const addon = fileURLToPath(
  new URL("../../../bindings/node/axton-node.node", import.meta.url),
);
const prelude = `
import { createRequire } from "node:module";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const { Bridge } = await import(${JSON.stringify(bridgeModule)});
const native = createRequire(import.meta.url)(${JSON.stringify(addon)});
const schema = JSON.parse(readFileSync(${JSON.stringify(fileURLToPath(schemaUrl))}, "utf8"));
const path = join(mkdtempSync(join(tmpdir(), "axton-bridge-exit-")), "client.sqlite");
`;

test("an outstanding task keeps the process alive until it settles", async () => {
  const { code, stdout, stderr } = await script(`${prelude}
const { bridge } = await Bridge.open(native, { path, schema });
await bridge.task(${JSON.stringify(create("x", "kept"))});
const row = await bridge.task(${JSON.stringify(read("x"))});
console.log("read " + row.text);
`);
  assert.equal(code, 0, stderr);
  assert.equal(stdout.trim(), "read kept");
});

test("an idle open client does not keep the process alive", async () => {
  const started = Date.now();
  const { code, stdout, stderr } = await script(`${prelude}
await Bridge.open(native, { path, schema });
console.log("opened");
`);
  assert.equal(code, 0, stderr);
  assert.equal(stdout.trim(), "opened");
  assert.ok(Date.now() - started < 15000);
});
