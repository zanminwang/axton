import test from "node:test";
import assert from "node:assert/strict";
import { httpTransport } from "./bindings/transport.mts";
import { createServerConnection as reactNativeConnection } from "../client-react-native/bindings/live.mts";
for (const [name, make] of [
  ["JS", (options) => httpTransport(options)],
  ["React Native", (options) => reactNativeConnection(options).push],
]) {
  test(`${name} forwards sync routes unchanged and refuses others`, async () => {
    const seen = [];
    const original = globalThis.fetch;
    globalThis.fetch = async (url, init) => {
      seen.push([url, init.body, init.headers.authorization]);
      return new Response("native answer");
    };
    try {
      const transport = make({ url: "http://server", token: "credential" });
      for (const route of ["handshake", "push", "pull", "action", "fetch"])
        assert.equal(await transport(route, "frozen"), "native answer");
      assert.deepEqual(seen, [
        ["http://server/sync/handshake", "frozen", "Bearer credential"],
        ["http://server/sync/mutations", "frozen", "Bearer credential"],
        ["http://server/sync/pull", "frozen", "Bearer credential"],
        ["http://server/sync/actions", "frozen", "Bearer credential"],
        ["http://server/sync/fetch", "frozen", "Bearer credential"],
      ]);
      await assert.rejects(transport("load", "frozen"), /unknown route load/);
      assert.equal(seen.length, 5, "nothing was posted for an unknown route");
    } finally {
      globalThis.fetch = original;
    }
  });
}
