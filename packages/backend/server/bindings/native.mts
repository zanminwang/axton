import { createRequire } from "node:module";
export type Native = {
  processLive(
    config: string,
    owner: string,
    request: string,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  processDelivery(
    config: string,
    owner: string,
    request: string,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  processRead(
    config: string,
    owner: string,
    request: string,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  handshake(
    config: string,
    owner: string,
    request: string,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  serverMaterializationId(config: string, projectionGeneration: string): string;
  validateMutationBatch?(config: string, request: string): string;
  processBatchMember?(
    config: string,
    owner: string,
    request: string,
    ordinal: number,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  encodeBatchAcknowledgement?(request: string, results: string[]): string;
  processBatchGroup?(
    config: string,
    owner: string,
    request: string,
    start: number,
    count: number,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  settleExternal(
    config: string,
    settlement: string,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  validateConfig(config: string): void;
  /** Negotiates and opens the socket's `Subscriptions`; answers `{handle, actions}` JSON. */
  negotiateLive(
    config: string,
    owner: string,
    request: string,
    callback: (request: string) => Promise<string>,
  ): Promise<string>;
  /** Applies one `LiveEvent` JSON to the session and answers its `LiveAction[]` JSON. */
  liveEvent(handle: number, event: string): string;
  /** Forgets the session; idempotent. */
  liveClose(handle: number): void;
};
/**
 * A failure reported by the native engine. `code` is the stable machine name
 * transports and applications should branch on; `message` is for people and
 * may be reworded; `details` carries the fields a code promises (only
 * `mutation_version_unsupported` has any: `ordinal`, `name`, `version`).
 */
export class EngineError extends Error {
  readonly code: string;
  readonly details: Record<string, unknown> | undefined;
  constructor(
    code: string,
    message: string,
    details?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "EngineError";
    this.code = code;
    this.details = details;
  }
}
/** The native addon carries the engine error as JSON in the error message. */
function engineError(error: unknown): unknown {
  if (error instanceof EngineError) return error;
  const text =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "";
  if (!text.startsWith("{")) return error;
  try {
    const parsed = JSON.parse(text);
    if (
      parsed &&
      typeof parsed === "object" &&
      typeof parsed.code === "string" &&
      typeof parsed.message === "string"
    ) {
      const details =
        parsed.details && typeof parsed.details === "object"
          ? (parsed.details as Record<string, unknown>)
          : undefined;
      return new EngineError(parsed.code, parsed.message, details);
    }
  } catch {
    // Not an engine error; leave it as received.
  }
  return error;
}
/** Wrap every native function so its failures surface as `EngineError`. */
function typedNative(native: Native): Native {
  type Async =
    | "negotiateLive"
    | "processDelivery"
    | "processRead"
    | "handshake"
    | "processLive"
    | "settleExternal";
  type Sync = "validateConfig" | "liveEvent" | "liveClose";
  const wrap =
    <K extends Async>(key: K) =>
    (...args: Parameters<Native[K]>): ReturnType<Native[K]> =>
      (native[key] as (...a: Parameters<Native[K]>) => ReturnType<Native[K]>)(
        ...args,
      ).catch((error: unknown) => {
        throw engineError(error);
      }) as ReturnType<Native[K]>;
  const wrapSync =
    <K extends Sync>(key: K) =>
    (...args: Parameters<Native[K]>): ReturnType<Native[K]> => {
      try {
        return (
          native[key] as (...a: Parameters<Native[K]>) => ReturnType<Native[K]>
        )(...args);
      } catch (error) {
        throw engineError(error);
      }
    };
  return {
    ...Object.fromEntries(
      ["processBatchGroup"]
        .filter((key) => typeof (native as any)[key] === "function")
        .map((key) => [
          key,
          (...args: any[]) =>
            (native as any)[key](...args).catch((error: unknown) => {
              throw engineError(error);
            }),
        ]),
    ),
    processDelivery: wrap("processDelivery"),
    processRead: wrap("processRead"),
    handshake: wrap("handshake"),
    processLive: wrap("processLive"),
    serverMaterializationId: (config: string, projectionGeneration: string) =>
      native.serverMaterializationId(config, projectionGeneration),
    ...(native.validateMutationBatch
      ? {
          validateMutationBatch: (config: string, request: string) => {
            try {
              return native.validateMutationBatch!(config, request);
            } catch (error) {
              throw engineError(error);
            }
          },
        }
      : {}),
    ...(native.processBatchMember
      ? {
          processBatchMember: (
            ...args: Parameters<NonNullable<Native["processBatchMember"]>>
          ) =>
            native.processBatchMember!(...args).catch((error) => {
              throw engineError(error);
            }),
        }
      : {}),
    ...(native.encodeBatchAcknowledgement
      ? {
          encodeBatchAcknowledgement: (request: string, results: string[]) => {
            try {
              return native.encodeBatchAcknowledgement!(request, results);
            } catch (error) {
              throw engineError(error);
            }
          },
        }
      : {}),
    settleExternal: wrap("settleExternal"),
    validateConfig: wrapSync("validateConfig"),
    negotiateLive: wrap("negotiateLive"),
    liveEvent: wrapSync("liveEvent"),
    liveClose: wrapSync("liveClose"),
  };
}

export function loadNative(native?: Native): Native {
  return typedNative(
    native ?? (createRequire(import.meta.url)("@axtonjs/native") as Native),
  );
}
