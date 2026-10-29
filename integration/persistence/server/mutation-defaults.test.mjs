import test from "node:test";
import assert from "node:assert/strict";
import { createBackend } from "../../../packages/backend/server/index.mts";

function fixture({ nativeGroups = true, adapterGroups = true, size } = {}) {
  const members = [],
    groups = [];
  const native = {
    validateConfig() {},
    serverMaterializationId05() {
      return "test-context";
    },
    validateMutationBatch(_config, wire) {
      return wire;
    },
    async processBatchMember(_config, _owner, _wire, ordinal) {
      members.push(ordinal);
      return String(ordinal);
    },
    encodeBatchAcknowledgement(_wire, results) {
      return JSON.stringify(results);
    },
  };
  if (nativeGroups)
    native.processBatchGroup = async (
      _config,
      _owner,
      _wire,
      ordinal,
      length,
    ) => {
      groups.push([ordinal, length]);
      return JSON.stringify(
        Array.from({ length }, (_, i) => String(ordinal + i)),
      );
    };
  const backend = createBackend({
    config: { schema: { enums: [], models: [], actions: [] } },
    native,
    database: {
      mutationCommitGroups: adapterGroups,
      transaction: async (body) => body({}),
      persistence: () => ({
        call: async () => {
          throw Error("unexpected host call");
        },
      }),
    },
    authenticate: () => "alice",
    loaders: {},
    protocol5: {
      authorizeStream: () => true,
      ...(size === undefined ? {} : { experimentalMutationGroupSize: size }),
    },
  });
  return { backend, members, groups };
}
const wire = JSON.stringify({
  mutations: Array.from({ length: 10 }, () => ({})),
});
test("supported backends default to eight; explicit one and unsupported capabilities retain member commits", async () => {
  for (const [options, expectedGroups] of [
    [
      {},
      [
        [0, 8],
        [8, 2],
      ],
    ],
    [{ size: 1 }, []],
    [{ nativeGroups: false }, []],
    [{ adapterGroups: false }, []],
  ]) {
    const f = fixture(options);
    assert.deepEqual(
      JSON.parse(await f.backend.push("alice", wire)),
      Array.from({ length: 10 }, (_, i) => String(i)),
    );
    assert.deepEqual(f.groups, expectedGroups);
    assert.equal(f.members.length, expectedGroups.length ? 0 : 10);
  }
});
test("an explicit unsupported group size is refused rather than silently reduced", () => {
  assert.throws(
    () => fixture({ nativeGroups: false, size: 8 }),
    /native Mutation commit groups unavailable/,
  );
  assert.throws(
    () => fixture({ adapterGroups: false, size: 8 }),
    /database adapter does not support Mutation commit groups/,
  );
});
