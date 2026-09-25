// Test wrapper around the To-do backend: a disposable database, one proxy per phone,
// and a control endpoint for network faults and result inspection.
import { writeFile } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import { connect as netConnect, type Socket } from "node:net";
import { createExample } from "../../../examples/todo/server.mts";

const app = await createExample();
await app.initialize();
const started = await app.listen(0);
const target = new URL(started.url);

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
        if (dropFirstPush && req.url === "/sync/mutations" && response.statusCode === 200) {
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
    if (!online) {
      socket.end("HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n");
      return;
    }
    const upstream = netConnect(Number(target.port), target.hostname, () => {
      const headers = Object.entries(req.headers).flatMap(([name, value]) =>
        Array.isArray(value) ? value.map((v) => `${name}: ${v}`) : [`${name}: ${value}`],
      );
      upstream.write(`${req.method} ${req.url} HTTP/1.1\r\n${headers.join("\r\n")}\r\n\r\n`);
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
    if (req.method === "POST" && req.url === "/alice/offline") alice.setOnline(false);
    if (req.method === "POST" && req.url === "/alice/online") alice.setOnline(true);
    res.setHeader("content-type", "application/json");
    res.end(
      JSON.stringify({
        handlerCalls: app.handlerCalls,
        droppedResponses: alice.droppedResponses,
        rows: await app.db.todo.findMany({ orderBy: { id: "asc" } }),
      }),
    );
  } catch (error) {
    res.writeHead(500);
    res.end(String(error));
  }
});
await new Promise<void>((resolve) => control.listen(0, "127.0.0.1", resolve));
// A subscription starts at the first head its handshake acknowledges
// ([#150](https://github.com/zanminwang/axton/issues/150)), so the seeds
// published at startup are not loaded by subscribing. Until
// [#151](https://github.com/zanminwang/axton/issues/151) gives the app an
// explicit `bootstrap()`, this harness publishes them again on a timer: whenever
// a phone's session becomes live, the next publication delivers them to it. The
// rows never change, so nothing else in the run depends on the interval.
let publishing = false;
const reseed = setInterval(() => {
  if (publishing) return;
  publishing = true;
  void app
    .publishSeeds()
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
  await app.close();
}
for (const signal of ["SIGINT", "SIGTERM"] as const) process.once(signal, () => void close());
