import { Book, Entry, createBackend, devAuth, type Loaders, type Mutations } from "./backend.ts";
type Tx = { rows: Map<string, object> };
// Transactional Mutation enqueue: the handler receives only the declared
// business args.
export const mutations: Mutations<Tx> = {
  async publishEntry({ ctx, args }) { ctx.tx.rows.set(args.entry.id, args.entry); ctx.invalidate.entry(args.entry); return { published: { id: args.entry.id } }; },
  async rename({ ctx, args }) { ctx.tx.rows.set(args.id, { title: args.title }); ctx.invalidate.entry({id:args.id}); },
};
export const loaders: Loaders<Tx> = {
  entry: {
    async v1({ ids }) { return ids.map((id) => ({ ...id, title: "old", note: null, at: new Date(0), status: "active" })); },
    async v2({ ids }) { return ids.map((id) => ({ ...id, title: "new", note: null, at: new Date(0), tags: [], status: "active" })); },
  },
  async book({ ids }) { return ids.map(() => null); },
  async comment({ ids }) { return ids.map(() => null); },
  async counter({ ids }) { return ids.map(() => null); },
  async composition({ ids }) { return ids.map(() => null); },
  async draft({ ids }) { return ids.map(() => null); },
  // A composite identity keeps every component, DateTime included.
  async placement({ ids }) { return ids.map((id) => ({ ...id, label: id.at.toISOString() })); },
};
/** A native entry point this check never reaches. */
const unreached = async (): Promise<string> => { throw new Error("not reached by this check"); };
export const backend = createBackend<Tx>({
  database: { transaction: async (body) => body({ rows: new Map() }), persistence: () => ({ call: async () => null }) },
  authenticate: devAuth(),
  authorizeStream:()=>true,
  mutations,
  queries: {readEntry: async () => ({entry:null})},
  loaders,
  native: { validateConfig() {}, serverMaterializationId: () => 'test-materialization', processLive: unreached, processDelivery: unreached, processRead: unreached, handshake: unreached, settleExternal: unreached, negotiateLive: async () => '', liveEvent: () => '[]', liveClose() {} },
});
// An external write declares through the same handles and answers its own value.
export const external: Promise<number> = backend.transaction(async ({ tx, streams: scope, invalidate: touch }) => { tx.rows.set("b", {}); touch.book({ id: "b" }); scope(["c"]).track.book({ id: "b" }); return tx.rows.size; });
// A write in a transaction the application owns declares through the same handles; the wake is called after that transaction commits.
export const owned: Promise<() => void> = backend.publish({ rows: new Map() }, ({ invalidate: touch, streams: scope }) => { touch.book({ id: "b" }); scope(["c"]).track.book({ id: "b" }); });
