// Subscription handles on the mobile host: the shared runtime with the React
// Native transaction scope and the native carrier, so identity and the
// committed status are the same behaviour Node sees and nothing about
// subscriptions is platform-specific
// ([#150](https://github.com/zanminwang/axton/issues/150)). The package entry
// point itself pulls in `expo-modules-core`, so this suite assembles the client
// from the same pieces, with no network configured.
import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { Transaction } from "../../../packages/client-react-native/transaction.mts";
import { createClient } from "../../../packages/client-js/runtime.mts";
const native = createRequire(import.meta.url)(
  "../../../bindings/node/axton-node.node",
);
const Client = createClient(native, Transaction, () => {
  throw Error("network not configured");
});

test("the mobile host shares the runtime subscription handles and their offline status", async () => {
  const directory = await mkdtemp(join(tmpdir(), "axton-rn-subscriptions-"));
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
  try {
    const [first, second] = await Promise.all([
      client.scopes.subscribe("project:123"),
      client.scopes.subscribe("project:123"),
    ]);
    assert.equal(first, second, "concurrent calls obtain one cached handle");
    assert.equal(first.scope, "project:123");
    assert.deepEqual(
      { ...first.status },
      { active: true, initialization: "pending", connection: "offline" },
      "no network is configured: durable intent with no boundary and no transport",
    );
    const seen = [];
    const stop = first.watch((status) => seen.push(status.connection));
    assert.deepEqual(seen, ["offline"], "the current snapshot arrives at once");
    stop();
    assert.equal(
      await client.subscribe("project:123"),
      first,
      "the retained spelling is the same registration",
    );
    await first.unsubscribe();
    assert.deepEqual(
      { ...first.status },
      { active: false, initialization: "pending", connection: "stopped" },
    );
    assert.deepEqual(
      (await client.syncState()).channels,
      [],
      "the registration is gone",
    );
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});
