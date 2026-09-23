import {
  AheadReport,
  startConnection,
  startLiveLane,
  type Connection,
  type ConnectionOptions,
  type LiveLane,
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
import type { ServerOptions, ServerConnection } from "./live.mts";
import { Events } from "./events.mts";

/** Hosts share the Rust action executor and supply only their carrier, transaction scope and network. */
export function createClient<
  Tx extends {
    finish(): Promise<void>;
    runCallback<T>(body: () => Promise<T>): Promise<T>;
    inCallback(): boolean;
  },
>(
  native: { clientCall(request: string): Promise<string> },
  Transaction: new (send: (request: RecordValue) => Promise<any>) => Tx,
  createServerConnection: (options: ServerOptions) => ServerConnection,
) {
  return class Client {
    #live: LiveLane | undefined;
    #syncing: Promise<void> | undefined;
    #tasks: Promise<void> | undefined;
    #connection: Connection | undefined;
    #connecting = false;
    #started: Promise<void> | undefined;
    #closing: Promise<void> | undefined;
    #handle: number;
    #closed = false;
    #tail: Promise<unknown> = Promise.resolve();
    #activePublicTx: Tx | undefined;
    #events = new Events();
    readonly clientId: string;
    private constructor(handle: number, id: string) {
      this.#handle = handle;
      this.clientId = id;
    }
    static async open(options: {
      path: string;
      schema: object;
      migration?: { defaults?: RecordValue; replayPull?: boolean };
      /** Rebuild at once when the schema is incompatible, leaving unsent work in the old file. */
      discardPending?: boolean;
    }) {
      const result = JSON.parse(
        await native.clientCall(strictJson({ op: "open", ...options })),
      ).value;
      return new Client(result.handle, result.clientId);
    }
    #exclusive<T>(body: () => Promise<T>): Promise<T> {
      const work = this.#tail.then(body);
      this.#tail = work.catch(() => {});
      return work;
    }
    async #send(request: RecordValue): Promise<any> {
      if (this.#closed) throw Error("client_closed");
      const result = JSON.parse(
        await native.clientCall(
          strictJson({ ...request, handle: this.#handle }),
        ),
      );
      if (result.changed) this.#events.emit("change");
      return result.value;
    }
    transaction<T>(body: (tx: Tx) => Promise<T>) {
      return this.#exclusive(async () => {
        await this.#send({ op: "begin" });
        const tx = new Transaction((request) => this.#send(request));
        try {
          this.#activePublicTx = tx;
          let result: T;
          try {
            result = await tx.runCallback(() => body(tx));
          } finally {
            this.#activePublicTx = undefined;
          }
          await tx.finish();
          await this.#send({ op: "commit" });
          this.#events.emit("work");
          return result;
        } catch (error) {
          await tx.finish().catch(() => {});
          await this.#send({ op: "rollback" }).catch(() => {});
          throw error;
        }
      });
    }
    read(model: string, identity: object): Promise<RecordValue | null> {
      return this.#exclusive(() =>
        this.#send({ op: "read", key: { model, identity } }),
      );
    }
    query(model: string, where: RecordValue = {}): Promise<RecordValue[]> {
      return this.#exclusive(() =>
        this.#send({ op: "query", model, filter: where }),
      );
    }
    readSql(sql: string, parameters: unknown[] = []): Promise<RecordValue[]> {
      return this.#exclusive(() => this.#send({ op: "sql", sql, parameters }));
    }
    querySpec(model: string, query: QuerySpec = {}): Promise<RecordValue[]> {
      return this.#exclusive(() =>
        this.#send({ op: "querySpec", model, query }),
      );
    }
    related(
      model: string,
      identity: object,
      relation: string,
    ): Promise<RecordValue | null> {
      return this.#exclusive(() =>
        this.#send({ op: "related", key: { model, identity }, relation }),
      );
    }
    referencing(
      model: string,
      identity: object,
      source: string,
      relation: string,
    ): Promise<RecordValue[]> {
      return this.#exclusive(() =>
        this.#send({
          op: "referencing",
          key: { model, identity },
          source,
          relation,
        }),
      );
    }
    mutate(mutation: object): Promise<number> {
      if (this.#activePublicTx?.inCallback())
        return Promise.reject(Error("transaction_active"));
      return this.#submitMutation(mutation);
    }
    #submitMutation(mutation: object): Promise<number> {
      return this.#exclusive(async () => {
        await this.#send({ op: "begin" });
        try {
          const ordinal = (await this.#send({
            op: "enqueue",
            mutation,
          })) as number;
          await this.#send({ op: "commit" });
          this.#events.emit("work");
          return ordinal;
        } catch (error) {
          try {
            await this.#send({ op: "rollback" });
          } catch (rollbackError) {
            throw new AggregateError(
              [error, rollbackError],
              "mutation submission and rollback failed",
            );
          }
          throw error;
        }
      });
    }
    subscribe(channel: string) {
      return this.#setChannel(channel, true);
    }
    unsubscribe(channel: string) {
      return this.#setChannel(channel, false);
    }
    /** The live session is abandoned at once; once the change commits, Rust starts one for the new channel set. */
    #setChannel(channel: string, subscribed: boolean) {
      this.#live?.cancel();
      return this.#exclusive(() =>
        this.#send({ op: "channel", channel, subscribed }).then(
          (value) => {
            this.#events.emit("channels");
            this.#events.emit("work");
            return value;
          },
          (error) => {
            this.#events.emit("channels");
            throw error;
          },
        ),
      );
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
          this.#exclusive(() =>
            this.#send({
              op: "connection",
              event,
              now: Date.now(),
              entropy: Math.floor(Math.random() * 0x100000000),
            }),
          );
        const connection = await startConnection(
          (event) => control(event),
          (t) => this.#runSync(t, options.onError),
          live.push,
          driverOptions,
        );
        const streaming = await startLiveLane(
          (event) =>
            this.#exclusive(() =>
              this.#send({
                op: "live",
                ...event,
                now: Date.now(),
                entropy: Math.floor(Math.random() * 0x100000000),
              }),
            ),
          live,
          driverOptions,
          () => void connection.wake().catch(options.onError ?? (() => {})),
        );
        this.#live = streaming;
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
            this.#events.off("work", wake);
            this.#events.off("channels", channels);
            await Promise.all([streaming.close(), connection.close()]);
            if (this.#connection === result) {
              this.#connection = undefined;
              this.#live = undefined;
            }
          },
        };
        this.#connection = result;
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
        await this.#exclusive(() =>
          this.#send({ op: "startSync", pushOnly: true }),
        );
        for (;;) {
          const action = await this.#exclusive(() =>
            this.#send({ op: "next" }),
          );
          if (action === null) return;
          const response = await transport(action.kind, action.body);
          const reports = (await this.#exclusive(() =>
            this.#send({ op: "complete", response: JSON.parse(response) }),
          )) as ReportDetails[];
          // What the receipt or page could not apply; the client stays
          // consistent and the application hears about each one.
          for (const report of reports) onError?.(new AheadReport(report));
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
          const task = await this.#exclusive(() =>
            this.#send({ op: "task", handlers: names }),
          );
          if (task === null) return;
          let error: string | null = null;
          try {
            await handlers[String(task.name)]!(task.arguments as RecordValue);
          } catch (thrown) {
            error = reason(thrown);
          }
          await this.#exclusive(() =>
            this.#send({ op: "outcome", key: task.key, error }),
          );
          this.#events.emit("work");
        }
      };
      this.#tasks = run().finally(() => {
        this.#tasks = undefined;
      });
      return this.#tasks;
    }
    freeze(): Promise<string | null> {
      return this.#exclusive(() => this.#send({ op: "freeze" }));
    }
    acknowledge(sequence: number, receipt: object) {
      return this.#exclusive(() =>
        this.#send({ op: "ack", sequence, receipt }),
      );
    }
    applyPull(page: object) {
      return this.#exclusive(() => this.#send({ op: "pull", page }));
    }
    /** The client's sync state, or one record's when `model` and `identity` are given. */
    syncState(): Promise<ClientSyncState>;
    syncState(model: string, identity: object): Promise<ModelSyncState>;
    syncState(model?: string, identity?: object) {
      return this.#exclusive(() =>
        model === undefined
          ? this.#send({ op: "status" })
          : this.#send({ op: "recordStatus", key: { model, identity } }),
      );
    }
    /**
     * Leave an incompatible database behind and open a fresh file for the
     * schema this client asked for. Refused while unsent mutations remain
     * unless `discardPending`; the report says what the old file keeps.
     */
    rebuild(
      options: { discardPending?: boolean } = {},
    ): Promise<RebuildReport> {
      return this.#exclusive(() =>
        this.#send({ op: "rebuild", ...options }).then((value) => {
          this.#events.emit("change");
          this.#events.emit("work");
          return value;
        }),
      );
    }
    pendingTasks(): Promise<RecordValue[]> {
      return this.#exclusive(() => this.#send({ op: "tasks" }));
    }
    setReadiness(key: string, state: "ready" | "pending" | "failed") {
      return this.#exclusive(() =>
        this.#send({ op: "readiness", key, state }).then((value) => {
          this.#events.emit("work");
          return value;
        }),
      );
    }
    drop(ordinal: number) {
      return this.#exclusive(() =>
        this.#send({ op: "drop", ordinal }).then((value) => {
          this.#events.emit("work");
          return value;
        }),
      );
    }
    dismissRejection(ordinal: number) {
      return this.#exclusive(() => this.#send({ op: "dismiss", ordinal }));
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
      return (this.#closing ??= this.#finishClose());
    }
    async #finishClose(): Promise<void> {
      await this.#started;
      await this.#connection?.close();
      await this.#exclusive(async () => {
        if (this.#closed) return;
        try {
          await this.#send({ op: "close" });
        } finally {
          this.#closed = true;
          this.#events.removeAllListeners();
        }
      });
    }
  };
}

/** The text a failed prerequisite keeps: the error's message, or the thrown value. */
function reason(thrown: unknown): string {
  if (thrown instanceof Error && thrown.message) return thrown.message;
  return String(thrown);
}
