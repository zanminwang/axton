import { Diagnostics, type AxtonTraceRecord } from "../api/diagnostics.mts";
import type {
  BackendOptions,
  Loader,
  MutationHandler,
  QueryHandler,
} from "../api/backend.mts";
import type { BackendDescriptor } from "../api/schema.mts";
import type { RuntimeStream, RuntimeLoadStream } from "../api/stream.mts";
import type { HostRequest } from "./host-contract.mts";
import {
  effectsFor,
  loadEffectsFor,
  bootstrapEffectsFor,
  lowerFirst,
} from "./effects.mts";
import type { Native } from "./native.mts";
import { Session } from "./session.mts";
import { isRetryableTransactionError } from "./retryable.mts";
export interface HostDependencies<T> {
  mutationGroupSize: number;
  diagnostics: Diagnostics;
  options: BackendOptions<T>;
  native: Native;
  config: string;
  descriptor: BackendDescriptor;
  schemaModels: NonNullable<NonNullable<BackendDescriptor["schema"]>["models"]>;
  actionTable: Map<
    string,
    NonNullable<NonNullable<BackendDescriptor["schema"]>["actions"]>[number]
  >;
  actionHandlers: Map<string, MutationHandler<T> | QueryHandler<T>>;
  loaderTable: Map<string, Loader<T>>;
  createEffects: ReturnType<typeof effectsFor>;
  createLoadEffects: ReturnType<typeof loadEffectsFor>;
  createBootstrapEffects: ReturnType<typeof bootstrapEffectsFor>;
  refusal(error: unknown): { rejection: string } | { error: string };
  onError(error: unknown): void;
}
/** JSON cannot represent nonfinite values or undefined array items. Never turn either into null. */
function encodeCallbackJson(value: unknown): string {
  return JSON.stringify(value, (_key, item) => {
    if (typeof item === "bigint") {
      const number = Number(item);
      if (!Number.isSafeInteger(number))
        throw new Error("bigint outside safe integer range");
      return number;
    }
    if (typeof item === "number" && !Number.isFinite(item))
      throw new Error("nonfinite callback value");
    if (item === undefined) throw new Error("undefined callback value");
    return item;
  });
}
/** Decode an operation value from its wire form into the API view (a DateTime becomes a Date). */
function decodeActionValue(type: any, value: unknown): unknown {
  if (value == null) return value;
  if (type?.kind === "list")
    return (value as unknown[]).map((item) =>
      decodeActionValue(type.element, item),
    );
  if (type?.name === "dateTime") return new Date(value as string);
  return value;
}
function decodeActionRecord(
  value: unknown,
  model: { fields?: { name: string; type: unknown }[] },
): unknown {
  if (value == null) return value;
  const record = value as Record<string, unknown>;
  for (const field of model.fields ?? [])
    if (Object.hasOwn(record, field.name))
      record[field.name] = decodeActionValue(field.type, record[field.name]);
  return record;
}
/** Current binding is trusted by the engine; selectors remain explicit. */
function scopedStreams<S extends RuntimeLoadStream>(
  effects: { stream: (names: string | readonly string[]) => S },
  context: { stream: string },
): {
  stream: S & ((names: string | readonly string[]) => S);
  streams: (names: readonly string[]) => S;
} {
  const current = effects.stream(context.stream);
  const stream = Object.assign(
    (names: string | readonly string[]) => effects.stream(names),
    current,
  );
  return {
    stream,
    streams: (names: readonly string[]) => effects.stream(names),
  };
}
export function createHost<T>(
  dependencies: HostDependencies<T>,
  tx: T,
  session: Session,
): (request: string) => Promise<string> {
  const {
    mutationGroupSize,
    options,
    diagnostics,
    native,
    config,
    descriptor,
    schemaModels,
    actionTable,
    actionHandlers,
    loaderTable,
    createEffects,
    createLoadEffects,
    createBootstrapEffects,
    refusal,
    onError,
  } = dependencies;
  const callbackJson = (value: unknown) =>
    diagnostics.syncSpan("host.encode", {}, () => encodeCallbackJson(value));
  const storage = options.database.persistence(
    tx,
    options.diagnostics ? diagnostics : undefined,
  );
  const execute = (req: HostRequest) =>
    session.track(async () => {
      let result: unknown;
      // `savepoint`, `rollback` and `release` are answered by the persistence
      // and also bookkept here, so each one does both.
      if (req.op === "savepoint") session.savepoint(req.ordinal);
      if (req.op === "rollback") session.rollback(req.ordinal);
      if (req.op === "release") session.release(req.ordinal);
      if (req.op === "protocol05" && req.request.op === "admit") {
        const context = req.request.context as { stream: string };
        result =
          !!options.protocol5 &&
          (await options.protocol5.authorizeStream(
            String(req.request.owner),
            context.stream,
            tx,
          ));
      } else if (
        req.op === "protocol05" &&
        req.request.op === "handleBootstrap05"
      ) {
        const effects = createBootstrapEffects();
        try {
          await options.bootstrap?.({
            ctx: {
              tx,
              userId: req.request.owner,
              callId: `bootstrap:${req.request.storeId}`,
              stream: Object.assign(
                (names: string | readonly string[]) => effects.stream(names),
                effects.stream(req.request.stream),
              ),
              streams: (names: readonly string[]) => effects.stream(names),
            },
          });
          if (effects.failure()) throw effects.failure()!.error;
          result = { declarations: effects.tracking() };
        } finally {
          effects.close();
        }
      } else if (req.op === "handleAction") {
        if (mutationGroupSize > 1) session.publicationScope = req.callId;
        const action = actionTable.get(`${req.name}:${req.version}`);
        const handler = actionHandlers.get(`${req.name}:${req.version}`);
        if (!action || !handler)
          throw new Error(`Missing handler ${req.name} v${req.version}`);
        const args = { ...req.arguments };
        for (const input of action.inputs ?? []) {
          if (input.kind === "value") {
            const type = input.list
              ? { kind: "list", element: input.type }
              : input.type;
            args[input.name] = decodeActionValue(type, args[input.name]);
            continue;
          }
          if (!input.model) continue;
          const model =
            action.input?.models?.find(
              (candidate: any) => candidate.name === input.model,
            ) ??
            schemaModels.find((candidate) => candidate.name === input.model);
          if (!model) throw new Error(`Missing Action model ${input.model}`);
          // The engine infers each operand as an input target; the handler
          // only sees the decoded record.
          const shape = (value: unknown): unknown =>
            value === null || value === undefined
              ? null
              : decodeActionRecord(value, model);
          const value = args[input.name];
          args[input.name] =
            input.cardinality === "list"
              ? (value as unknown[]).map(shape)
              : shape(value);
        }
        // Bound Queries may track explicitly, but cannot invalidate or write.
        // Legacy Queries expose no declaration handles.
        const query = (action.kind ?? "mutation") === "query";
        const effects = query
          ? req.context
            ? createLoadEffects()
            : undefined
          : createEffects();
        try {
          const outputs = await diagnostics.span(
            "handler.invoke",
            { callId: req.callId, name: req.name },
            async () =>
              handler({
                ctx: effects
                  ? {
                      tx,
                      userId: req.owner,
                      callId: req.callId,
                      ...(req.context
                        ? scopedStreams(effects, req.context)
                        : { stream: effects.stream }),
                      ...(!query
                        ? {
                            invalidate: (
                              effects as ReturnType<typeof createEffects>
                            ).invalidate,
                          }
                        : {}),
                    }
                  : { tx, userId: req.owner, callId: req.callId },
                args,
              } as Parameters<MutationHandler<T>>[0]),
          );
          // A caught declaration refusal still fails the entire read. Never
          // settle the prefix collected before an overflow or invalid input.
          if (query && effects) {
            const failure = (
              effects as ReturnType<typeof createLoadEffects>
            ).failure();
            if (failure) throw failure.error;
          }
          result = {
            outputs: outputs === undefined ? {} : outputs,
            ...(effects
              ? query
                ? {
                    changes: [],
                    declarations: (
                      effects as ReturnType<typeof createLoadEffects>
                    ).tracking(),
                  }
                : (effects as ReturnType<typeof createEffects>).settlement()
              : { changes: [], declarations: [] }),
          };
        } catch (error) {
          if (isRetryableTransactionError(error)) throw error;
          const answer = refusal(error);
          if ("error" in answer) throw error;
          result = answer;
        } finally {
          effects?.close();
        }
      } else if (req.op === "load") {
        // Dispatch is by model name and contract version; a version that
        // was not registered is a defect, never another version's loader.
        const loader = loaderTable.get(`${req.model}:${req.version}`);
        if (!loader)
          throw new Error(`Missing loader ${req.model} v${req.version}`);
        const loaderModel =
          (descriptor.models ?? []).find(
            (model: any) =>
              model.name === req.model && model.version === req.version,
          ) ?? schemaModels.find((model) => model.name === req.model);
        const call = {
          ids: (req.identities as any[]).map((identity) =>
            loaderModel ? decodeActionRecord(identity, loaderModel) : identity,
          ),
          tx,
          userId: req.owner,
        };
        // Explicit business refusal is data. Other thrown errors abort the
        // acceptance/read transaction; the caller's retry loop keeps the
        // original infrastructure error.
        let refused: { rejection: string } | { error: string } | undefined;
        let rows: unknown;
        try {
          const hook = options.loaderHooks?.[lowerFirst(req.model)];
          if (req.mode !== "canonical" && hook) {
            const effects = createEffects();
            try {
              await hook.prepareForViewer({
                ...call,
                streams: effects.stream,
                invalidate: effects.invalidate,
              });
            } finally {
              effects.close();
            }
            await native.settleExternal05!(
              config,
              JSON.stringify(effects.settlement()),
              createHost(dependencies, tx, session),
            );
          }
          rows = req.mode === "prepare" ? [] : await loader(call);
        } catch (error) {
          if (isRetryableTransactionError(error)) throw error;
          const answer = refusal(error);
          if ("error" in answer) throw error;
          refused = answer;
        }
        if (refused) return callbackJson(refused);
        // An answer JSON cannot carry faithfully is a failed read, never a
        // null: the engine retries the records one by one, so only the
        // record whose row is broken fails.
        let reason: string | undefined;
        let answer = "";
        if (!Array.isArray(rows)) reason = "a non-array result";
        else if (rows.some((value) => value === undefined))
          reason = "an undefined entry";
        else
          try {
            answer = callbackJson(rows);
          } catch (error) {
            reason = error instanceof Error ? error.message : String(error);
          }
        if (reason === undefined) return answer;
        const invalid = new Error(
          `invalid loader answer for ${req.model} v${req.version}: ${reason}`,
        );
        onError(invalid);
        return callbackJson({ error: invalid.message });
      } else {
        // Everything the persistence owns, plus anything this build does not
        // know: an operation added to the contract without an arm here is a
        // compile error, not a silent forward.
        switch (req.op) {
          case "protocol05":
          case "head":
          case "savepoint":
          case "rollback":
          case "release":
          case "readTracking":
          case "guardRecords":
          case "lockStreams":
          case "applyStreamMembers":
            break;
          default: {
            const unreachable: never = req;
            void unreachable;
          }
        }
        const stored =
          req.op === "protocol05" &&
          req.request.op === "applyStreamMembers" &&
          session.publicationScope
            ? {
                ...req,
                request: {
                  ...req.request,
                  publicationScope: session.publicationScope,
                },
              }
            : req;
        if (
          stored.op === "protocol05" &&
          stored.request.op === "saveResults" &&
          !options.database.mutationResultGroups
        ) {
          const group = stored.request;
          for (const [index, outcome] of group.results.entries()) {
            await storage.call({
              op: "protocol05",
              request: {
                op: "saveResult",
                storeId: group.storeId,
                batchId: group.batchId,
                ordinal: group.ordinal + index,
                count: group.count,
                result: outcome,
              },
            });
          }
          result = null;
        } else {
          result = await storage.call(stored);
        }
        // Every position that survives its savepoint wakes the stream's
        // subscribers after commit; `rollback` restores the set it snapshot.
        const publication =
          req.op === "protocol05" && req.request.op === "applyStreamMembers"
            ? req.request
            : req.op === "applyStreamMembers"
              ? req
              : undefined;
        if (publication)
          for (const delta of publication.deltas as {
            publish: boolean;
            stream: string;
          }[])
            if (delta.publish) session.touched.add(delta.stream);
      }
      return callbackJson(result);
    });
  return diagnostics.bind((raw: string) => {
    const req = diagnostics.syncSpan("host.decode", {}, () =>
      JSON.parse(raw),
    ) as
      | HostRequest
      | { op: "trace"; records: AxtonTraceRecord[]; dropped: number };
    if (req.op === "trace") {
      diagnostics.native(req.records, req.dropped);
      return Promise.resolve("{}");
    }
    const name =
      req.op === "protocol05" ? `host.${req.request.op}` : `host.${req.op}`;
    const attributes =
      req.op === "load"
        ? { model: req.model, rows: req.identities.length, mode: req.mode }
        : req.op === "handleAction"
          ? { callId: req.callId, mutation: req.name }
          : {};
    return diagnostics.span(name, attributes, () => execute(req));
  });
}
