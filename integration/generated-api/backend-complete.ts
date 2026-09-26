import { Book, createBackend, devAuth, type Handlers, type Loaders } from "./backend.ts";
type Tx = { rows: Map<string, object> };
export const handlers: Handlers<Tx> = {
  // An ordinary write: the changed record is stamped and read back without any publication.
  async createEntry({ input, tx }) { tx.rows.set(input.entry.id, input.entry); },
  editEntry: {
    async v1({ publish }) { publish({ channel: "c" }); },
    // Publication of the change set includes a record added after the call; explicit records publish only those.
    async v2({ input, changes, publish }) { publish({ channel: "c" }); changes.add(input.entry); publish({ channel: "audit", records: [input.entry] }); },
  },
  removeEntries: {
    async v1({ input, publish }) { publish({ channel: "c", records: input.entries }); },
    async v2({ input, publish }) { publish({ channel: "c", records: input.entries }); },
  },
  async addBook({ input, changes, publish }) { changes.add(Book({ id: input.book.id })); publish({ channel: "c", records: [] }); },
  async addComment({ publish }) { publish({ channel: "c" }); },
  // Handlers receive the client-expanded create: defaulted fields are present and required (#27).
  async addDraft({ input, tx }) { const { id, created, body }: { id: string; created: Date; body: string } = input.draft; tx.rows.set(id, { created, body }); },
};
export const loaders: Loaders<Tx> = {
  entry: {
    async v1({ ids }) { return ids.map((id) => ({ ...id, title: "old", note: null, at: new Date(0), status: "active" })); },
    async v2({ ids }) { return ids.map((id) => ({ ...id, title: "new", note: null, at: new Date(0), tags: [], status: "active" })); },
  },
  async book({ ids }) { return ids.map(() => null); },
  async comment({ ids }) { return ids.map(() => null); },
  async counter({ ids }) { return ids.map(() => null); },
  async draft({ ids }) { return ids.map(() => null); },
};
export const backend = createBackend<Tx>({
  database: { transaction: async (body) => body({ rows: new Map() }), persistence: () => ({ call: async () => null }) },
  authenticate: devAuth(),
  handlers,
  loaders,
  native: { validateConfig() {}, processPush: async () => "", processAction: async () => "", processPull: async () => "", settleExternal: async () => "", negotiateLive: async () => "", pullLive: async () => "", liveEvent: () => "[]", liveClose() {} },
});
