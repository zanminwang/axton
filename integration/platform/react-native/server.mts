import {
  PrismaClient,
  type Prisma,
} from "../../e2e/fixtures/round-trip/node_modules/@prisma/client/default.js";
import { readFile, writeFile } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import { connect as netConnect, type Socket } from "node:net";
import { prisma } from "../../../packages/postgres/index.mts";
import {
  createBackend,
  devAuth,
  MutationRejected,
  type Handlers,
  type Loaders,
} from "./generated/backend.ts";

const db = new PrismaClient();
const calls = { add: 0, edit: 0 };
const handlers: Handlers<Prisma.TransactionClient> = {
  async addEntry({ input, tx, publish }) {
    calls.add++;
    await tx.entry.create({ data: input.entry });
    publish({ channel: "book:demo" });
  },
  async edit({ input, tx, publish }) {
    calls.edit++;
    if (input.entry.patch.text === "reject")
      throw new MutationRejected("entry.denied");
    await tx.entry.update({
      where: input.entry.identity,
      data: input.entry.patch,
    });
    publish({ channel: "book:demo" });
  },
};
const loaders: Loaders<Prisma.TransactionClient> = {
  async entry({ ids, tx }) {
    return Promise.all(
      ids.map((identity) => tx.entry.findUnique({ where: identity })),
    );
  },
};
const backend = createBackend({
  database: prisma(db),
  authenticate: devAuth(),
  handlers,
  loaders,
});
const migration = await readFile(
  new URL(
    "../../../packages/postgres/migration.sql",
    import.meta.url,
  ),
  "utf8",
);
for (const sql of migration
  .split(";")
  .map((x) => x.trim())
  .filter(Boolean))
  await db.$executeRawUnsafe(sql);
await db.$executeRawUnsafe(
  'CREATE TABLE IF NOT EXISTS "Entry" (id TEXT PRIMARY KEY,text TEXT NOT NULL,note TEXT)',
);
await backend.transaction(async ({ tx, changes, publish }) => {
  await tx.entry.upsert({
    where: { id: "entry-1" },
    create: { id: "entry-1", text: "seed", note: null },
    update: {},
  });
  changes.add({ model: "Entry", identity: { id: "entry-1" } });
  publish({ channel: "book:demo" });
});
const started = await backend.listen({ port: 0, host: "127.0.0.1" });
const target = new URL(started.url);

/**
 * TODO(#151): a subscription starts at the head its first handshake acknowledges
 * ([#150](https://github.com/zanminwang/axton/issues/150)), so the rows seeded
 * above are not loaded by subscribing. Until
 * [#151](https://github.com/zanminwang/axton/issues/151) gives the app an
 * explicit `bootstrap()`, this harness republishes them on a bounded timer (see
 * the `TODO(#151)` at `setInterval` below). A phone's live socket is an upgrade
 * through its proxy, counted here.
 */
let sessionUpgrades = 0;
let publishedAfterSession = 0;
/** Publications after a phone's socket opened, and for the whole run. */
const REPUBLISH_AFTER_SESSION = 3;
const REPUBLISH_LIMIT = 60;

async function proxy(dropFirstPush: boolean) {
  let online = true;
  let droppedResponses = 0;
  const sockets = new Set<Socket>();
  const server = createServer((req, res) => {
    if (!online) {
      res.writeHead(503);
      res.end("disconnected by test harness");
      return;
    }
    const upstream = httpRequest(
      {
        hostname: target.hostname,
        port: target.port,
        path: req.url,
        method: req.method,
        headers: req.headers,
      },
      (response) => {
        if (
          dropFirstPush &&
          req.url === "/sync/mutations" &&
          response.statusCode === 200
        ) {
          dropFirstPush = false;
          response.resume();
          response.on("end", () => {
            droppedResponses++;
            res.destroy();
          });
        } else {
          res.writeHead(response.statusCode ?? 502, response.headers);
          response.pipe(res);
        }
      },
    );
    upstream.on("error", () => {
      if (!res.headersSent) res.writeHead(502);
      res.end();
    });
    req.on("error", () => upstream.destroy());
    req.pipe(upstream);
  });
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (req, socket, head) => {
    // The HTTP server drops its own error listener on upgrade; a reset from the phone
    // (for example when the runner terminates it) must not crash the proxy.
    socket.on("error", () => {});
    // A phone is opening its live socket: the republish timer above is bounded by this.
    sessionUpgrades++;
    publishedAfterSession = 0;
    if (!online) {
      socket.end(
        "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n",
      );
      return;
    }
    const upstream = netConnect(Number(target.port), target.hostname, () => {
      const headers = Object.entries(req.headers).flatMap(([name, value]) =>
        Array.isArray(value)
          ? value.map((v) => `${name}: ${v}`)
          : [`${name}: ${value}`],
      );
      upstream.write(
        `${req.method} ${req.url} HTTP/1.1\r\n${headers.join("\r\n")}\r\n\r\n`,
      );
      if (head.length) upstream.write(head);
      socket.pipe(upstream);
      upstream.pipe(socket);
    });
    upstream.on("error", () => socket.destroy());
    socket.on("error", () => upstream.destroy());
    socket.on("close", () => upstream.destroy());
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    url: `http://127.0.0.1:${(server.address() as { port: number }).port}`,
    setOnline(value: boolean) {
      online = value;
      if (!online) for (const socket of sockets) socket.destroy();
    },
    get droppedResponses() {
      return droppedResponses;
    },
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
const alice = await proxy(true);
const bob = await proxy(false);
const control = createServer(async (req, res) => {
  try {
    if (req.method === "POST" && req.url === "/alice/offline")
      alice.setOnline(false);
    if (req.method === "POST" && req.url === "/alice/online")
      alice.setOnline(true);
    res.setHeader("content-type", "application/json");
    res.end(
      JSON.stringify({
        calls,
        droppedResponses: alice.droppedResponses,
        rows: await db.entry.findMany({ orderBy: { id: "asc" } }),
      }),
    );
  } catch (error) {
    res.writeHead(500);
    res.end(String(error));
  }
});
await new Promise<void>((resolve) => control.listen(0, "127.0.0.1", resolve));
// TODO(#151): stand-in for the explicit historical load. Republish the seeded
// rows once a second until a phone's session has been live for a few
// publications, and never more than REPUBLISH_LIMIT times in the run; the rows
// never change, so only the arrival of the seeded state depends on this.
let republishes = 0;
let publishing = false;
const reseed = setInterval(() => {
  if (publishing) return;
  if (
    republishes >= REPUBLISH_LIMIT ||
    (sessionUpgrades > 0 && publishedAfterSession >= REPUBLISH_AFTER_SESSION)
  ) {
    clearInterval(reseed);
    return;
  }
  republishes++;
  if (sessionUpgrades > 0) publishedAfterSession++;
  publishing = true;
  void backend
    .transaction(async ({ changes, publish }) => {
      changes.add({ model: "Entry", identity: { id: "entry-1" } });
      publish({ channel: "book:demo" });
    })
    .catch((error) => console.log("seed:", String(error)))
    .finally(() => {
      publishing = false;
    });
}, 1000);
reseed.unref();
const ports = {
  alice: alice.url,
  bob: bob.url,
  control: `http://127.0.0.1:${(control.address() as { port: number }).port}`,
};
await writeFile(process.env.AXTON_SMOKE_PORTS!, JSON.stringify(ports));
console.log(JSON.stringify(ports));
let closing = false;
async function close() {
  if (closing) return;
  closing = true;
  clearInterval(reseed);
  await Promise.all([alice.close(), bob.close()]);
  await new Promise<void>((resolve) => control.close(() => resolve()));
  await started.close();
  await db.$disconnect();
}
for (const signal of ["SIGINT", "SIGTERM"] as const)
  process.once(signal, () => void close());
