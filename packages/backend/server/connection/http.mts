import type { IncomingMessage, ServerResponse } from "node:http";
import { EngineError } from "../bindings/native.mts";
import { ADMISSION_HEADER, type Admission } from "./admission.mts";

/**
 * Engine codes with a client-visible HTTP status. Every other failure is a
 * server-side defect: reported to `onError` and answered `500 {code: "server"}`.
 */
const HTTP_STATUS_BY_CODE: Readonly<Record<string, number>> = {
  "delivery.capacity": 413,
  "request.invalid": 400,
  context_mismatch: 409,
  "stream.forbidden": 403,
  "store.binding": 403,
  "principal.invalid": 400,
  "batch.conflict": 409,
  "batch.sequence": 409,
  "batch.progress": 409,
  "page.capacity": 413,
  constraint_group_capacity: 413,
  "protocol.unsupported": 426,
  model_version_unsupported: 409,
};
export interface HttpBackend {
  handshake?(owner: string, request: Uint8Array | string): Promise<string>;
  materialize?(owner: string, request: Uint8Array | string): Promise<string>;
  push(owner: string, request: Uint8Array | string): Promise<string>;
  pull(owner: string, request: Uint8Array | string): Promise<string>;
  action(owner: string, request: Uint8Array | string): Promise<string>;
  fetch(owner: string, request: Uint8Array | string): Promise<string>;
}
export function createHttpHandler(options: {
  backend: HttpBackend;
  admit: Admission;
  maxBodyBytes?: number;
  onError?: (error: unknown) => void;
}): (request: IncomingMessage, response: ServerResponse) => Promise<void> {
  return async (request, response) => {
    const send = (status: number, value: unknown) => {
      if (response.destroyed || response.writableEnded) return;
      response.writeHead(status, {
        "content-type": "application/json; charset=utf-8",
        "cache-control": "no-store",
      });
      response.end(typeof value === "string" ? value : JSON.stringify(value));
    };
    const path = request.url?.split("?")[0];
    if (
      path !== "/sync/mutations" &&
      path !== "/sync/pull" &&
      path !== "/sync/actions" &&
      path !== "/sync/fetch" &&
      path !== "/sync/materialize" &&
      path !== "/sync/handshake"
    ) {
      send(404, { code: "not_found" });
      return;
    }
    if (request.method !== "POST") {
      response.setHeader("allow", "POST");
      send(405, { code: "method_not_allowed" });
      return;
    }
    try {
      const { owner, refusal: refused } = await options.admit(request);
      if (refused) {
        if (response.destroyed || response.writableEnded) return;
        response.writeHead(refused.status, {
          "content-type": "application/json; charset=utf-8",
          "cache-control": "no-store",
          [ADMISSION_HEADER]: "refused",
        });
        response.end(refused.body);
        return;
      }
      if (!owner?.trim()) {
        send(401, { code: "unauthenticated" });
        return;
      }
      const chunks: Buffer[] = [];
      let size = 0;
      for await (const chunk of request) {
        const buffer = Buffer.from(chunk);
        size += buffer.length;
        if (size > (options.maxBodyBytes ?? 1_048_576)) {
          send(413, { code: "request_too_large" });
          return;
        }
        chunks.push(buffer);
      }
      const bytes = Buffer.concat(chunks);
      let body: unknown;
      try {
        body = JSON.parse(
          new TextDecoder("utf-8", { fatal: true }).decode(bytes),
        );
      } catch {
        send(400, { code: "request.invalid" });
        return;
      }
      if (body === null || typeof body !== "object" || Array.isArray(body)) {
        send(400, { code: "request.invalid" });
        return;
      }
      const result = await (path === "/sync/handshake"
        ? options.backend.handshake!(owner, bytes)
        : path === "/sync/materialize"
          ? options.backend.materialize!(owner, bytes)
          : path === "/sync/mutations"
            ? options.backend.push(owner, bytes)
            : path === "/sync/actions"
              ? options.backend.action(owner, bytes)
              : path === "/sync/fetch"
                ? options.backend.fetch(owner, bytes)
                : options.backend.pull(owner, bytes));
      send(200, result);
    } catch (error) {
      const status =
        error instanceof EngineError
          ? HTTP_STATUS_BY_CODE[error.code]
          : undefined;
      if (error instanceof EngineError && status !== undefined) {
        send(status, { code: error.code, ...(error.details ?? {}) });
        return;
      }
      options.onError?.(error);
      send(500, { code: "server" });
    }
  };
}
