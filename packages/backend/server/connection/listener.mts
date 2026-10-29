import { createServer } from "node:http";
import type { Admission } from "./admission.mts";
import { createHttpHandler, type HttpBackend } from "./http.mts";
import { attachLive, type LiveBackend } from "./live.mts";

/** Own listener admission, in-flight work and shutdown; backend operations own transactions. */
export function createListener(options: {
  backend: HttpBackend & LiveBackend;
  admit: Admission;
  ready(): Promise<void>;
  onError(error: unknown): void;
}) {
  const { backend, admit, ready, onError } = options;
  return async ({
    port,
    host = "127.0.0.1",
  }: {
    port: number;
    host?: string;
  }) => {
    await ready();
    let stopping = false;
    const requests = new Set<Promise<void>>();
    const handler = createHttpHandler({ backend, admit, onError });
    const server = createServer((request, response) => {
      if (stopping) {
        response.writeHead(503, { connection: "close" });
        response.end();
        return;
      }
      const work = handler(request, response);
      requests.add(work);
      void work.then(
        () => requests.delete(work),
        () => requests.delete(work),
      );
    });
    const live = attachLive(server, {
      backend,
      admit,
      onError,
    });
    await new Promise<void>((resolve, reject) => {
      const onError = (error: Error) => reject(error);
      server.once("error", onError);
      server.listen(port, host, () => {
        server.off("error", onError);
        resolve();
      });
    });
    const address = server.address();
    const actual = typeof address === "object" && address ? address.port : port;
    const urlHost =
      host === "0.0.0.0"
        ? "127.0.0.1"
        : host === "::"
          ? "localhost"
          : host.includes(":")
            ? `[${host}]`
            : host;
    let closing: Promise<void> | undefined;
    return {
      url: `http://${urlHost}:${actual}`,
      close: () => {
        stopping = true;
        return (closing ??= (async () => {
          const drained = await Promise.allSettled([live.close(), ...requests]);
          server.closeIdleConnections();
          const closed = await Promise.allSettled([
            new Promise<void>((resolve, reject) =>
              server.close((error) => (error ? reject(error) : resolve())),
            ),
          ]);
          const failure = [...drained, ...closed].find(
            (result) => result.status === "rejected",
          );
          if (failure?.status === "rejected") throw failure.reason;
        })());
      },
    };
  };
}
