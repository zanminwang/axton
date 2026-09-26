import {
  AxtonReport,
  directFailure,
  startConnection,
  startDownlinkLane,
  type Connection,
  type DirectConnection,
  type ConnectionOptions,
  type ReportDetails,
  type Transport,
} from "./connection.mts";
export type { Connection, ConnectionOptions } from "./connection.mts";
/** A rejection retained in the local inbox until dismissed. */
export type Rejection = {
  ordinal: number;
  code: string;
  [key: string]: unknown;
};
/** One queued mutation touching a record. `diverged` is set when its edit could not be replayed over newer authority. */
export type PendingMutation<Name extends string = string> = {
  ordinal: number;
  name: Name;
  phase: "queued" | "frozen";
  prerequisites: { key: string; state: "ready" | "pending" | "failed" }[];
  diverged?: boolean;
};
/** One record's sync state: what is still pending for it and what was rejected. */
export type ModelSyncState<Name extends string = string> = {
  pending: PendingMutation<Name>[];
  rejections: Rejection[];
};
/** The whole client's sync state: a local snapshot, not a network probe. */
/** What a rebuild left in the old database file. */
export type RebuildReport = {
  oldFile: string;
  newFile: string;
  reason: string;
  leftPending: number;
  leftDirect: number;
  abandonedCalls: { callId: string; frozen: boolean }[];
};
/** The open-time schema check: whether this open rebuilt, or is waiting to. */
export type SchemaState = {
  rebuilt: boolean;
  /** The incompatible file is still in use because it holds unsent work. */
  pending: {
    oldFile: string;
    reason: string;
    pending: number;
    direct: number;
  } | null;
  lastRebuild: RebuildReport | null;
};
export type ClientSyncState = {
  clientId: string;
  pending: number;
  beforeImages: number;
  cursors: Record<string, number>;
  channels: string[];
  rejections: Rejection[];
  schema: SchemaState;
};
import { strictJson, type QuerySpec, type RecordValue } from "./values.mts";
import { Bridge, reportCallbackError, type NativeCarrier } from "./bridge.mts";
export type { NativeCarrier } from "./bridge.mts";
import type { ServerOptions, ServerConnection } from "./live.mts";
import { Events } from "./events.mts";
import {
  Subscriptions,
  type BootstrapRun,
  type Subscription,
  type SubscriptionState,
} from "./subscriptions.mts";
export type {
  BootstrapPhase,
  BootstrapStatus,
  Subscription,
  SubscriptionState,
  SubscriptionStatus,
} from "./subscriptions.mts";
import {
  ActionRegistry,
  CallError,
  actionError,
  assertCallOptions,
  onceControls,
  type Call,
  type CallOptions,
  type QueryOptions,
} from "./actions.mts";

/**
 * The native command field for an Action's store option, beside its args.
 * Once controls never reach this seam: Mutations and `enqueue` refuse them.
 */
function storeOption(options?: CallOptions): { store?: unknown } {
  assertCallOptions(options);
  return options?.store === undefined ? {} : { store: options.store };
}
type DirectOutcome = {
  status: string;
  result?: unknown;
  code?: string;
  execution?: string;
};
/** Decode one caller's result from a terminal direct outcome. */
function decodeOutcome<T>(
  outcome: DirectOutcome | undefined,
  decode: (value: unknown) => T,
): T {
  if (!outcome) throw new CallError("action.observation_failed");
  if (outcome.status === "failed")
    throw new CallError(
      outcome.code ?? "action.failed",
      outcome.execution === "rejected" ? "rejected" : "unknown",
    );
  try {
    return decode(outcome.result);
  } catch (cause) {
    throw new CallError("action.observation_failed", "unknown", cause);
  }
}
/**
 * One active once request shared by every caller Rust joins to it. It holds
 * the raw outcome text only until settlement; each caller parses and decodes
 * its own copy, so no caller can mutate another's result.
 */
class QueryFlight {
  readonly promise: Promise<string>;
  resolve!: (outcome: string) => void;
  reject!: (error: unknown) => void;
  constructor() {
    this.promise = new Promise((resolve, reject) => {
      this.resolve = resolve;
      this.reject = reject;
    });
    // Settled callers observe it; an unobserved rejection is not an error.
    this.promise.catch(() => {});
  }
}
/** Report an application callback's failure without changing what was committed. */
const reportActionCallbackError = reportCallbackError;

/**
 * Hosts share the Rust-owned client runtime and supply only their carrier,
 * transaction scope and network. Every command is a task of that runtime
 * ([#134](https://github.com/zanminwang/axton/issues/134)): it queues,
 * schedules and completes them; this class adapts them to typed APIs.
 */
export function createClient<
  Tx extends {
    finish(): Promise<void>;
    runCallback<T>(body: () => Promise<T>): Promise<T>;
    inCallback(): boolean;
    direct(operation: object): Promise<void>;
  },
>(
  native: NativeCarrier,
  Transaction: new (
    send: (command: RecordValue, scope?: string) => Promise<any>,
  ) => Tx,
  createServerConnection: (options: ServerOptions) => ServerConnection,
) {
  return class Client {
    #syncing: Promise<void> | undefined;
    #tasks: Promise<void> | undefined;
    #connection: Connection | undefined;
    #direct: DirectConnection | undefined;
    #directOnError: ((error: unknown) => void) | undefined;
    #completionListeners = new Set<(completion: any) => void>();
    #actions = new ActionRegistry();
    /** Active once flights by Rust flight ID; removed at their terminal step. */
    #queryFlights = new Map<string, QueryFlight>();
    #connecting = false;
    #started: Promise<void> | undefined;
    #closing: Promise<void> | undefined;
    #bridge: Bridge;
    #closed = false;
    #activePublicTx: Tx | undefined;
    #events = new Events();
    /** Subscription handles by persistent identity, and the status they publish. */
    #subscriptions = new Subscriptions({
      subscribe: (scope) =>
        this.#bridge.task({
          kind: "scopeSubscribe",
          scope,
        }) as Promise<SubscriptionState>,
      state: (scope) =>
        this.#bridge.task({
          kind: "scopeState",
          scope,
        }) as Promise<SubscriptionState | null>,
      // Registration and the state read of one identity's durable load: tasks
      // of the runtime's one ordinary queue, so calls for one Scope keep their
      // order ([#151](https://github.com/zanminwang/axton/issues/151)).
      requestBootstrap: (scope, subscriptionId) =>
        this.#bridge.task({
          kind: "scopeBootstrap",
          scope,
          subscriptionId,
        }) as Promise<BootstrapRun>,
      bootstrapState: (scope, subscriptionId) =>
        this.#bridge.task({
          kind: "scopeBootstrapState",
          scope,
          subscriptionId,
        }) as Promise<BootstrapRun>,
      remove: async (scope, subscriptionId) =>
        (
          await this.#bridge.task({
            kind: "scopeUnsubscribe",
            scope,
            subscriptionId,
          })
        ).removed === true,
      removeScope: async (scope) => {
        await this.#bridge.task({
          kind: "channel",
          channel: scope,
          subscribed: false,
        });
      },
      // A committed membership change wakes the lanes; Rust decides what it
      // means for the socket.
      committed: () => {
        this.#events.emit("channels");
        this.#events.emit("work");
      },
      report: reportActionCallbackError,
    });
    readonly clientId: string;
    private constructor(bridge: Bridge, id: string) {
      this.#bridge = bridge;
      this.clientId = id;
      // A committed local transaction: watchers re-run their queries.
      bridge.on("changed", () => this.#events.emit("change"));
    }
    static async open(options: {
      path: string;
      schema: object;
      migration?: { defaults?: RecordValue; replayPull?: boolean };
      /** Rebuild at once when the schema is incompatible, leaving unsent work in the old file. */
      discardPending?: boolean;
    }) {
      const { bridge, opened } = await Bridge.open(native, options);
      return new Client(bridge, opened.clientId);
    }
    /**
     * Run `body` as the callback of a local transaction the runtime owns. The
     * callback's commands carry its capability; its return value stays here
     * and is answered only after the runtime confirmed the commit.
     */
    async transaction<T>(body: (tx: Tx) => Promise<T>): Promise<T> {
      let result!: T;
      await this.#bridge.transaction(async (transactionId) => {
        const tx = new Transaction((command, scope) =>
          this.#bridge.transactionCommand(transactionId, scope, command),
        );
        try {
          this.#activePublicTx = tx;
          try {
            result = await tx.runCallback(() => body(tx));
          } finally {
            this.#activePublicTx = undefined;
          }
          await tx.finish();
        } catch (error) {
          // No unawaited command outlives the callback that issued it.
          await tx.finish().catch(() => {});
          throw error;
        }
      });
      this.#events.emit("work");
      return result;
    }
    read(model: string, identity: object): Promise<RecordValue | null> {
      return this.#bridge.task({ kind: "read", key: { model, identity } });
    }
    query(model: string, where: RecordValue = {}): Promise<RecordValue[]> {
      return this.#bridge.task({ kind: "query", model, filter: where });
    }
    readSql(sql: string, parameters: unknown[] = []): Promise<RecordValue[]> {
      return this.#bridge.task({ kind: "sql", sql, parameters });
    }
    querySpec(model: string, query: QuerySpec = {}): Promise<RecordValue[]> {
      return this.#bridge.task({ kind: "querySpec", model, query });
    }
    related(
      model: string,
      identity: object,
      relation: string,
    ): Promise<RecordValue | null> {
      return this.#bridge.task({
        kind: "related",
        key: { model, identity },
        relation,
      });
    }
    referencing(
      model: string,
      identity: object,
      source: string,
      relation: string,
    ): Promise<RecordValue[]> {
      return this.#bridge.task({
        kind: "referencing",
        key: { model, identity },
        source,
        relation,
      });
    }
    mutate(mutation: object): Promise<number> {
      if (this.#activePublicTx?.inCallback())
        return Promise.reject(Error("transaction_active"));
      return this.#submitMutation(mutation);
    }
    /** One standalone Model write in its own local transaction. */
    direct(operation: object): Promise<void> {
      if (this.#activePublicTx?.inCallback())
        return Promise.reject(Error("transaction_active"));
      return this.transaction(async (tx) => {
        await tx.direct(operation);
      });
    }
    /** Submit durable work and register its observer before the work wake. */
    async invokeAction<T>(
      name: string,
      version: number,
      args: object,
      decode: (value: unknown) => T,
      options?: CallOptions,
    ): Promise<Call<T>> {
      this.#actions.assertSupported();
      let call: Call<T> | undefined;
      try {
        await this.submitAction(
          name,
          version,
          args,
          (callId) => {
            call = this.#actions.register(callId, decode);
          },
          options,
        );
      } catch (error) {
        throw actionError(error);
      }
      return call!;
    }
    /** Execute a direct Action and decode its committed result. */
    async invokeDirectAction<T>(
      name: string,
      version: number,
      args: object,
      decode: (value: unknown) => T,
      options?: CallOptions,
    ): Promise<T> {
      let applied: {
        completions: {
          outcome: {
            status: string;
            result?: unknown;
            code?: string;
            execution?: string;
          };
        }[];
      };
      try {
        applied = await this.callAction(name, version, args, options);
      } catch (error) {
        throw actionError(error);
      }
      return decodeOutcome(applied.completions[0]?.outcome, decode);
    }
    /**
     * Execute a direct Query. Without `once` it is exactly
     * [`invokeDirectAction`]: a fresh request that reads and writes no
     * snapshot. With `once`, Rust decides: a saved result is decoded without
     * any request or Model write, an active request is joined, or a new one
     * is executed and its successful result saved with its authority.
     */
    async invokeQuery<T>(
      name: string,
      version: number,
      args: object,
      decode: (value: unknown) => T,
      options?: QueryOptions,
    ): Promise<T> {
      const { once, refresh } = onceControls(options);
      const call: CallOptions =
        options?.store === undefined ? {} : { store: options.store };
      if (!once)
        return this.invokeDirectAction(name, version, args, decode, call);
      let outcome: string;
      try {
        outcome = await this.#queryOnce(name, version, args, refresh, call);
      } catch (error) {
        throw actionError(error);
      }
      return decodeOutcome(JSON.parse(outcome), decode);
    }
    /**
     * Discard the saved once results of one Query argument set, every store
     * variant, in a local transaction. Needs no network; an older request
     * still in flight cannot save its result afterwards.
     */
    async invalidateQuery(
      name: string,
      version: number,
      args: object,
    ): Promise<void> {
      try {
        if (this.#activePublicTx?.inCallback())
          throw Error("transaction_active");
        await this.#bridge.task({
          kind: "invalidateQueryOnce",
          name,
          version,
          args,
        });
      } catch (error) {
        throw actionError(error);
      }
    }
    /** The raw outcome text of a once call: cached, joined or fetched. */
    async #queryOnce(
      name: string,
      version: number,
      args: object,
      refresh: boolean,
      options: CallOptions,
    ): Promise<string> {
      if (this.#activePublicTx?.inCallback()) throw Error("transaction_active");
      // A decision that joins a flight completes after the decision that
      // fetches it: the runtime runs tasks in order and the bridge dispatches
      // their completions in order. Each caller registers or looks up its
      // flight in the continuation of this one await, so a joining caller
      // always finds the flight. Keep exactly one await before touching
      // `#queryFlights` here; checkpoint 3 of #134 moves the flight into Rust.
      const decided = await this.#bridge.task({
        kind: "queryOnce",
        name,
        version,
        args,
        refresh,
        ...storeOption(options),
      });
      if (decided.decision === "cached")
        return JSON.stringify({ status: "succeeded", result: decided.result });
      if (decided.decision === "join") {
        const flight = this.#queryFlights.get(decided.flightId);
        if (!flight) throw directFailure("action.execution_unknown");
        return flight.promise;
      }
      const direct = this.#direct;
      if (!direct) {
        await this.#bridge.task({
          kind: "failQueryOnce",
          flightId: decided.flightId,
        });
        throw directFailure("action.unavailable");
      }
      const flight = new QueryFlight();
      this.#queryFlights.set(decided.flightId, flight);
      void this.#runQueryFlight(
        {
          flightId: decided.flightId as string,
          body: decided.body as string,
          direct,
        },
        flight,
      );
      return flight.promise;
    }
    /** Execute one fetched flight and settle every caller joined to it. */
    async #runQueryFlight(
      fetch: { flightId: string; body: string; direct: DirectConnection },
      flight: QueryFlight,
    ): Promise<void> {
      const { flightId, body, direct } = fetch;
      /**
       * Release the Rust flight and fail its callers. The flight stays
       * registered until the release completed, so a caller the runtime
       * joined to it before the release still finds it.
       */
      const release = async (error: unknown) => {
        if (this.#queryFlights.get(flightId) === flight) {
          await this.#bridge
            .task({ kind: "failQueryOnce", flightId })
            .catch(() => {});
          if (this.#queryFlights.get(flightId) === flight)
            this.#queryFlights.delete(flightId);
        }
        flight.reject(error);
      };
      let response: string;
      try {
        response = await direct.requestAction(body);
      } catch (error) {
        return release(
          (error as { code?: string })?.code
            ? error
            : Object.assign(directFailure("action.execution_unknown"), {
                cause: error,
              }),
        );
      }
      let applied: { completions: any[]; reports: ReportDetails[] };
      let finishing = false;
      try {
        if (this.#direct !== direct)
          throw directFailure("action.execution_unknown");
        const parsed = JSON.parse(response);
        finishing = true;
        try {
          applied = await this.#bridge.task({
            kind: "finishQueryOnce",
            flightId,
            response: parsed,
          });
        } finally {
          // Rust retires the flight on every outcome of this command; a
          // caller joined before it still found the flight registered.
          if (this.#queryFlights.get(flightId) === flight)
            this.#queryFlights.delete(flightId);
        }
      } catch (error) {
        if (!finishing)
          return release(
            (error as { code?: string })?.code
              ? error
              : Object.assign(directFailure("action.execution_unknown"), {
                  cause: error,
                }),
          );
        return flight.reject(
          (error as { code?: string })?.code
            ? error
            : Object.assign(directFailure("action.execution_unknown"), {
                cause: error,
              }),
        );
      }
      this.#deliverCompletions(applied.completions);
      for (const report of applied.reports)
        try {
          this.#directOnError?.(new AxtonReport(report));
        } catch (error) {
          reportActionCallbackError(error);
        }
      const outcome = applied.completions[0]?.outcome;
      if (outcome) flight.resolve(JSON.stringify(outcome));
      else flight.reject(new CallError("action.observation_failed"));
    }
    /** Internal Action seam: register after local commit, synchronously before waking work. */
    submitAction(
      name: string,
      version: number,
      args: object,
      onCommitted?: (callId: string, ordinal: number) => void,
      options?: CallOptions,
    ): Promise<{ callId: string; ordinal: number }> {
      if (this.#activePublicTx?.inCallback())
        return Promise.reject(Error("transaction_active"));
      return (async () => {
        const submitted = (await this.#bridge.task({
          kind: "submitAction",
          name,
          version,
          args,
          ...storeOption(options),
        })) as { callId: string; ordinal: number };
        onCommitted?.(submitted.callId, submitted.ordinal);
        this.#events.emit("work");
        return submitted;
      })();
    }
    onActionCompletion(listener: (completion: any) => void): () => void {
      this.#completionListeners.add(listener);
      return () => this.#completionListeners.delete(listener);
    }
    #deliverCompletions(completions: any[]): void {
      // Finish every registered handle before an application diagnostic listener can throw.
      for (const completion of completions) this.#actions.complete(completion);
      for (const completion of completions)
        for (const listener of [...this.#completionListeners])
          try {
            listener(completion);
          } catch (error) {
            reportActionCallbackError(error);
          }
    }
    /** Direct network work holds no runtime task while it waits on the network. */
    async callAction(
      name: string,
      version: number,
      args: object,
      options?: CallOptions,
    ) {
      if (this.#activePublicTx?.inCallback()) throw Error("transaction_active");
      const direct = this.#direct;
      if (!direct) throw directFailure("action.unavailable");
      const prepared = (await this.#bridge.task({
        kind: "prepareAction",
        name,
        version,
        args,
        ...storeOption(options),
      })) as { callId: string; body: string };
      let response: string;
      try {
        response = await direct.requestAction(prepared.body);
      } catch (error) {
        if ((error as { code?: string })?.code) throw error;
        throw Object.assign(Error("action.execution_unknown"), {
          code: "action.execution_unknown",
          execution: "unknown" as const,
          cause: error,
        });
      }
      if (this.#direct !== direct)
        throw directFailure("action.execution_unknown");
      let applied: { completions: any[]; reports: ReportDetails[] };
      try {
        applied = await this.#bridge.task({
          kind: "applyActionResponse",
          body: prepared.body,
          response: JSON.parse(response),
        });
      } catch (error) {
        if ((error as { code?: string })?.code) throw error;
        throw Object.assign(directFailure("action.execution_unknown"), {
          cause: error,
        });
      }
      this.#deliverCompletions(applied.completions);
      for (const report of applied.reports)
        try {
          this.#directOnError?.(new AxtonReport(report));
        } catch (error) {
          reportActionCallbackError(error);
        }
      return applied;
    }
    /** One queued mutation; the runtime enqueues it in its own transaction. */
    async #submitMutation(mutation: object): Promise<number> {
      const ordinal = (await this.#bridge.task({
        kind: "enqueue",
        mutation,
      })) as number;
      this.#events.emit("work");
      return ordinal;
    }
    /**
     * Register durable intent to follow `scope` and answer with its handle. It
     * resolves when the local transaction commits: it awaits no
     * authentication, connection or acknowledgement, and the same Scope answers
     * with the same handle while its registration lives. The socket is never
     * cancelled here; the Downlink worker sees the committed change and
     * reconciles its own session.
     */
    subscribeScope(scope: string): Promise<Subscription> {
      return this.#subscriptions.subscribe(scope);
    }
    /** The Scope surface the generated `scopes` facade delegates to, with no logic of its own. */
    get scopes(): { subscribe(scope: string): Promise<Subscription> } {
      return { subscribe: (scope) => this.subscribeScope(scope) };
    }
    subscribe(channel: string): Promise<Subscription> {
      return this.subscribeScope(channel);
    }
    /** Remove whatever registration this Scope name has; its handle stops. */
    unsubscribe(channel: string): Promise<void> {
      return this.#subscriptions.unsubscribeScope(channel);
    }
    async connect(
      server: ServerOptions,
      options: ConnectionOptions = {},
    ): Promise<Connection> {
      if (this.#closed || this.#closing) throw Error("client_closed");
      if (this.#connecting || this.#connection)
        throw Error("connection already active");
      this.#connecting = true;
      let finished!: () => void;
      this.#started = new Promise<void>((resolve) => {
        finished = resolve;
      });
      try {
        if (
          !server ||
          typeof server !== "object" ||
          typeof server.url !== "string" ||
          !(
            typeof server.token === "string" ||
            typeof server.token === "function"
          )
        )
          throw Error("connect requires server: {url, token}");
        const live = createServerConnection(server);
        let refreshing: Promise<void> | undefined;
        const driverOptions = {
          ...options,
          ...(options.refreshAuth
            ? {
                refreshAuth: () =>
                  (refreshing ??= Promise.resolve()
                    .then(() => options.refreshAuth!())
                    .finally(() => {
                      refreshing = undefined;
                    })),
              }
            : {}),
        };
        const control = (event: string) =>
          this.#bridge.task({
            kind: "connection",
            event,
            now: Date.now(),
            entropy: Math.floor(Math.random() * 0x100000000),
          });
        const connection = await startConnection(
          (event) => control(event),
          (t) => this.#runSync(t, options.onError),
          live.push,
          driverOptions,
        );
        const streaming = await startDownlinkLane(
          (event) =>
            this.#bridge.task({
              kind: "downlink",
              ...event,
              now: Date.now(),
              entropy: Math.floor(Math.random() * 0x100000000),
            }),
          live,
          driverOptions,
          () => void connection.wake().catch(options.onError ?? (() => {})),
          (signal) => this.#subscriptions.signal(signal),
        );
        this.#subscriptions.attach();
        const channels = () => {
          void streaming.wake().catch(options.onError ?? (() => {}));
        };
        this.#events.on("channels", channels);
        const wake = () => {
          void connection.wake().catch(options.onError ?? (() => {}));
        };
        this.#events.on("work", wake);
        const result = {
          ...connection,
          pause: async () => {
            await Promise.all([streaming.pause(), connection.pause()]);
          },
          resume: async () => {
            await Promise.all([streaming.resume(), connection.resume()]);
          },
          wake: async () => {
            await Promise.all([streaming.wake(), connection.wake()]);
          },
          close: async () => {
            this.#direct = undefined;
            this.#directOnError = undefined;
            this.#subscriptions.detach();
            this.#events.off("work", wake);
            this.#events.off("channels", channels);
            await Promise.all([streaming.close(), connection.close()]);
            if (this.#connection === result) this.#connection = undefined;
          },
        };
        this.#connection = result;
        this.#direct = connection;
        this.#directOnError = options.onError;
        return result;
      } finally {
        this.#connecting = false;
        finished();
      }
    }
    #runSync(
      transport: Transport,
      onError?: (error: unknown) => void,
    ): Promise<void> {
      if (this.#syncing) return this.#syncing;
      const run = async () => {
        await this.#bridge.task({ kind: "startSync", pushOnly: true });
        for (;;) {
          const action = await this.#bridge.task({ kind: "next" });
          if (action === null) return;
          const response = await transport(action.kind, action.body);
          const applied = (await this.#bridge.task({
            kind: "complete",
            response: JSON.parse(response),
          })) as { reports: ReportDetails[]; completions: any[] };
          this.#deliverCompletions(applied.completions);
          // What the receipt or page could not apply; the client stays
          // consistent and the application hears about each one.
          for (const report of applied.reports)
            try {
              onError?.(new AxtonReport(report));
            } catch (error) {
              reportActionCallbackError(error);
            }
        }
      };
      this.#syncing = run().finally(() => {
        this.#syncing = undefined;
      });
      return this.#syncing;
    }
    runPrerequisites(
      handlers: Record<string, (arguments_: RecordValue) => Promise<void>>,
    ): Promise<void> {
      if (this.#tasks) return this.#tasks;
      // Rust picks the task and settles it; this loop only calls the handler.
      const names = Object.keys(handlers);
      const run = async () => {
        for (;;) {
          const task = await this.#bridge.task({
            kind: "task",
            handlers: names,
          });
          if (task === null) return;
          let error: string | null = null;
          try {
            await handlers[String(task.name)]!(task.arguments as RecordValue);
          } catch (thrown) {
            error = reason(thrown);
          }
          await this.#bridge.task({ kind: "outcome", key: task.key, error });
          this.#events.emit("work");
        }
      };
      this.#tasks = run().finally(() => {
        this.#tasks = undefined;
      });
      return this.#tasks;
    }
    freeze(): Promise<string | null> {
      return this.#bridge.task({ kind: "freeze" });
    }
    async acknowledge(sequence: number, receipt: object) {
      const applied = await this.#bridge.task({
        kind: "ack",
        sequence,
        receipt,
      });
      this.#deliverCompletions(applied.completions);
      return applied;
    }
    applyPull(page: object) {
      return this.#bridge.task({ kind: "pull", page });
    }
    /** The client's sync state, or one record's when `model` and `identity` are given. */
    syncState(): Promise<ClientSyncState>;
    syncState(model: string, identity: object): Promise<ModelSyncState>;
    syncState(model?: string, identity?: object) {
      return model === undefined
        ? this.#bridge.task({ kind: "status" })
        : this.#bridge.task({ kind: "recordStatus", key: { model, identity } });
    }
    /**
     * Leave an incompatible database behind and open a fresh file for the
     * schema this client asked for. Refused while unsent mutations remain
     * unless `discardPending`; the report says what the old file keeps.
     */
    rebuild(
      options: { discardPending?: boolean } = {},
    ): Promise<RebuildReport> {
      return this.#bridge
        .task({ kind: "rebuild", ...options })
        .then((value) => {
          // The replica that answered every handle is gone: no handle from
          // before it names a registration of the file this client now reads.
          this.#subscriptions.rebuilt();
          this.#deliverCompletions(
            (value.abandonedCalls ?? []).map(
              (abandoned: { callId: string; frozen: boolean }) => ({
                callId: abandoned.callId,
                outcome: {
                  status: "failed",
                  code: "abandoned",
                  execution: abandoned.frozen ? "unknown" : "rejected",
                },
              }),
            ),
          );
          this.#events.emit("change");
          this.#events.emit("work");
          return value;
        });
    }
    pendingTasks(): Promise<RecordValue[]> {
      return this.#bridge.task({ kind: "tasks" });
    }
    setReadiness(key: string, state: "ready" | "pending" | "failed") {
      return this.#bridge
        .task({ kind: "readiness", key, state })
        .then((value) => {
          this.#events.emit("work");
          return value;
        });
    }
    drop(ordinal: number) {
      return this.#bridge.task({ kind: "drop", ordinal }).then((value) => {
        this.#deliverCompletions(value.completions);
        this.#events.emit("work");
        return undefined;
      });
    }
    dismissRejection(ordinal: number) {
      return this.#bridge.task({ kind: "dismiss", ordinal });
    }
    watch(
      model: string,
      where: RecordValue = {},
      listener: (rows: RecordValue[]) => void,
      onError: (error: unknown) => void = () => {},
    ) {
      let previous: string | undefined;
      let closed = false;
      const refresh = () => {
        this.query(model, where)
          .then((rows) => {
            const value = strictJson(rows);
            if (!closed && value !== previous) {
              previous = value;
              listener(rows);
            }
          })
          .catch(onError);
      };
      this.#events.on("change", refresh);
      refresh();
      return () => {
        closed = true;
        this.#events.off("change", refresh);
      };
    }
    close(): Promise<void> {
      this.#actions.close();
      for (const flight of this.#queryFlights.values())
        flight.reject(new CallError("client.closed"));
      this.#queryFlights.clear();
      this.#subscriptions.close();
      return (this.#closing ??= this.#finishClose());
    }
    async #finishClose(): Promise<void> {
      await this.#started;
      await this.#connection?.close();
      try {
        await this.#bridge.close();
      } finally {
        this.#closed = true;
        this.#events.removeAllListeners();
      }
    }
  };
}

/** The text a failed prerequisite keeps: the error's message, or the thrown value. */
function reason(thrown: unknown): string {
  if (thrown instanceof Error && thrown.message) return thrown.message;
  return String(thrown);
}
