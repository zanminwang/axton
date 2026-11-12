import { createHash } from "node:crypto";
// Open offline unless the test explicitly supplies a network connection.
export const offlineNetwork = () => ({
  open() {},
  async push() {
    throw Error("offline");
  },
});
export async function openStore(Client, options) {
  return Client.open({ ...options, stream: options.stream ?? "User:viewer" });
}

const canonical = (value) =>
  Array.isArray(value)
    ? `[${value.map(canonical).join(",")}]`
    : value && typeof value === "object"
      ? `{${Object.keys(value)
          .sort()
          .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`)
          .join(",")}}`
      : JSON.stringify(value);
const context05 = (body) =>
  Object.fromEntries(
    ["protocol", "storeId", "stream", "materialization"].map((key) => [
      key,
      body[key],
    ]),
  );
const hash05 = (domain, value) =>
  createHash("sha256")
    .update(domain + "\0")
    .update(canonical(value))
    .digest("hex");
export function emptyHandshake(body) {
  return {
    protocol: 6,
    storeId: body.storeId,
    stream: body.stream,
    head: 0,
    projectionGeneration: "1",
  };
}
const fixtureExpiry = Date.now() + 300000;
export function delivery05(body, changes = [], head = body.through) {
  const through = body.through;
  const unit = { index: 0, through, changes };
  const part = {
    planId: `transport-${body.storeId}-${body.bootstrap ? "bootstrap" : "sync"}-${body.after}-${body.through}`,
    planDigest: "",
    unit: 0,
    part: 0,
    changes,
  };
  const manifest = {
    through,
    minimumCursor: changes.length
      ? Math.min(...changes.map((change) => change.cursor))
      : null,
    digest: hash05("axton:delivery-unit:5", unit),
    parts: [hash05("axton:delivery-part:5", { unit: 0, part: 0, changes })],
  };
  const header = {
    ...context05(body),
    planId: part.planId,
    digest: "",
    bootstrap: body.bootstrap ?? false,
    after: body.after,
    through,
    observedHead: head,
    expiresAt: fixtureExpiry,
    units: [manifest],
  };
  const { digest, ...unsigned } = header;
  header.digest = hash05("axton:delivery-plan:5", unsigned);
  part.planDigest = header.digest;
  return { header, parts: [part] };
}
export function read05(body, result = null, records = []) {
  return {
    ...context05(body),
    requestId: body.requestId,
    outcome: { kind: "succeeded", result },
    records,
  };
}
const request05 = (text) => {
  const request = typeof text === "string" ? JSON.parse(text) : text;
  if (request.protocol !== 6) throw Error("fixture requires protocol 6");
  return request;
};
export function emptyPull(text) {
  const request = request05(text);
  return JSON.stringify(request.materialization ? delivery05(request) : emptyHandshake(request));
}
export function emptyRead(text) {
  const request = request05(text);
  return JSON.stringify(read05(request, null,
    request.invocation.kind === "fetch" ? [{ key: request.invocation.key, cursor: null, state: null }] : []));
}
export function emptyMutation(text) {
  const request = request05(text);
  return JSON.stringify({
    ...context05(request), batchId: request.batchId, digest: request.digest,
    results: request.mutations.map(mutation => ({mutationId: mutation.id,
      outcome: {kind: "accepted", syncCursor: 0, materialization: request.materialization, result: null, targets: []}})),
  });
}
/**
 * A fake server for seeding delivered rows: the Handshake reports head 1 and
 * the Bootstrap delivers `records` (`{ model, identity, state }`) at cursor 1.
 * Use it with `deliver` only where a test needs a delivered, non-pending row.
 */
export const deliveringNetwork = (records) => () => ({
  open() {},
  async push(route, text) {
    const request = request05(text);
    if (route === "handshake")
      return JSON.stringify({ ...emptyHandshake(request), head: 1 });
    if (route !== "pull") throw Error("offline");
    const changes = request.bootstrap
      ? records.map(({ model, identity, state }) => ({
          kind: "record",
          cursor: 1,
          key: { model, identity },
          state,
        }))
      : [];
    return JSON.stringify(delivery05(request, changes, 1));
  },
});
/** Open the Store with `Client` over `deliveringNetwork`, take its Bootstrap, and close it. */
export async function deliver(Client, options) {
  const client = await openStore(Client, options);
  try {
    const connection = await client.connect({ url: "http://fixture", token: "fixture" });
    await client.bootstrap();
    await connection.close();
  } finally {
    await client.close();
  }
}
/**
 * An in-process fake server with a Stream log, for a test that needs
 * delivered rows and later delivered changes. Its Bootstrap delivers
 * `initial` (`{ model, identity, state }` each) at cursor 1; `send(rows)`
 * appends one unit and delivers it over every open socket, answering its
 * cursor. Uploads fail, so submitted work stays unsettled.
 */
export function streamServer(initial = []) {
  const record =
    (cursor) =>
    ({ model, identity, state }) => ({
      kind: "record",
      cursor,
      key: { model, identity },
      state,
    });
  const log = initial.length ? [initial.map(record(1))] : [];
  const sockets = new Set();
  let context;
  return {
    network: () => ({
      open(subscribe, signal, on) {
        sockets.add(on);
        signal.addEventListener("abort", () => sockets.delete(on), {
          once: true,
        });
        const handshake = { ...emptyHandshake(JSON.parse(subscribe)), head: log.length };
        void Promise.resolve().then(() => on.message(JSON.stringify(handshake)));
      },
      async push(route, text) {
        const request = request05(text);
        if (route === "handshake")
          return JSON.stringify({ ...emptyHandshake(request), head: log.length });
        if (route !== "pull") throw Error("offline");
        context = context05(request);
        const from = request.bootstrap ? 0 : request.after;
        return JSON.stringify(
          delivery05(request, log.slice(from, request.through).flat(), log.length),
        );
      },
    }),
    async send(rows) {
      log.push(rows.map(record(log.length + 1)));
      const through = log.length;
      const page = delivery05(
        { ...context, bootstrap: false, after: through - 1, through },
        log[through - 1],
        through,
      );
      for (const on of sockets) await on.message(JSON.stringify(page));
      return through;
    },
  };
}
