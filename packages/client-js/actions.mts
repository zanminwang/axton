/** A terminal Action error observed by the client. */
export class ActionError extends Error {
  readonly code: string;
  readonly execution: "rejected" | "unknown";
  override readonly cause: unknown;
  constructor(
    code: string,
    execution: "rejected" | "unknown" = "unknown",
    cause?: unknown,
  ) {
    super(code);
    this.name = "ActionError";
    this.code = code;
    this.execution = execution;
    this.cause = cause;
  }
}

export type ActionStatus = "pending" | "succeeded" | "failed";
export type ActionOutcome<T> =
  { result: T; error: null } | { result: undefined; error: ActionError };
/**
 * Invocation options, kept apart from business args. `store` selects which
 * explicit Model outputs also update local Models: omitted or `true` stores
 * all, `false` none, and a map overrides named outputs (unnamed ones stay
 * true). Results are the same either way.
 */
export type ActionOptions<K extends string = string> = {
  store?: boolean | Partial<Record<K, boolean>>;
};
export interface ActionCall<T> {
  readonly status: ActionStatus;
  wait(): Promise<ActionOutcome<T>>;
}

type Completion = {
  callId: string;
  outcome:
    | { status: "succeeded"; result: unknown }
    | { status: "failed"; code: string; execution: "rejected" | "unknown" };
};

class CallState<T> implements ActionCall<T> {
  status: ActionStatus = "pending";
  #outcome: ActionOutcome<T> | undefined;
  #promise: Promise<ActionOutcome<T>>;
  #resolve!: (value: ActionOutcome<T>) => void;
  #activate: () => void;
  constructor(activate: () => void) {
    this.#activate = activate;
    this.#promise = new Promise((resolve) => {
      this.#resolve = resolve;
    });
  }
  wait(): Promise<ActionOutcome<T>> {
    if (!this.#outcome) this.#activate();
    return this.#promise;
  }
  settle(outcome: ActionOutcome<T>): void {
    if (this.#outcome) return;
    this.#outcome = outcome;
    this.status = outcome.error === null ? "succeeded" : "failed";
    this.#resolve(outcome);
  }
}

type WeakState = { deref(): CallState<unknown> | undefined };
type WeakFactory = (state: CallState<unknown>) => WeakState;

/** Routes transient completions without retaining abandoned handles. */
export class ActionRegistry {
  #routes = new Map<
    string,
    { ref: WeakState; decode: (value: unknown) => unknown }
  >();
  #active = new Map<string, CallState<unknown>>();
  #weak: WeakFactory | null;
  #usesRuntimeWeak: boolean;
  #closed = false;
  constructor(weak?: WeakFactory | null) {
    this.#usesRuntimeWeak = weak === undefined;
    this.#weak = weak === undefined ? (state) => new WeakRef(state) : weak;
  }
  get routingCount(): number {
    return this.#routes.size;
  }
  get activeCount(): number {
    return this.#active.size;
  }
  assertSupported(): void {
    if (!this.#weak) throw new ActionError("action.unsupported_runtime");
    if (this.#usesRuntimeWeak) {
      try {
        if (
          typeof WeakRef !== "function" ||
          typeof new WeakRef({}).deref !== "function"
        )
          throw Error("WeakRef is unavailable");
      } catch (cause) {
        throw new ActionError("action.unsupported_runtime", "unknown", cause);
      }
    }
  }
  register<T>(callId: string, decode: (value: unknown) => T): ActionCall<T> {
    this.assertSupported();
    this.#sweep();
    const state = new CallState<T>(() => {
      this.#active.set(callId, state as CallState<unknown>);
    });
    if (this.#closed)
      state.settle({
        result: undefined,
        error: new ActionError("client.closed"),
      });
    else
      this.#routes.set(callId, {
        ref: this.#weak!(state as CallState<unknown>),
        decode,
      });
    return state;
  }
  complete(completion: Completion): void {
    this.#sweep();
    const route = this.#routes.get(completion.callId);
    const state = this.#active.get(completion.callId) ?? route?.ref.deref();
    if (!state) return;
    let outcome: ActionOutcome<unknown>;
    if (completion.outcome.status === "failed") {
      outcome = {
        result: undefined,
        error: new ActionError(
          completion.outcome.code,
          completion.outcome.execution,
        ),
      };
    } else {
      try {
        outcome = {
          result: route!.decode(completion.outcome.result),
          error: null,
        };
      } catch (cause) {
        outcome = {
          result: undefined,
          error: new ActionError("action.observation_failed", "unknown", cause),
        };
      }
    }
    state.settle(outcome);
    this.#routes.delete(completion.callId);
    this.#active.delete(completion.callId);
  }
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const [id, route] of this.#routes) {
      const state = this.#active.get(id) ?? route.ref.deref();
      state?.settle({
        result: undefined,
        error: new ActionError("client.closed"),
      });
    }
    this.#routes.clear();
    this.#active.clear();
  }
  #sweep(): void {
    for (const [id, route] of this.#routes) {
      if (!route.ref.deref() && !this.#active.has(id)) this.#routes.delete(id);
    }
  }
}

export function actionError(error: unknown): ActionError {
  if (error instanceof ActionError) return error;
  const value = error as { code?: unknown; execution?: unknown } | null;
  const code =
    typeof value?.code === "string"
      ? value.code
      : error instanceof Error && error.message === "transaction_active"
        ? "transaction_active"
        : "action.transport_failed";
  const execution =
    value?.execution === "rejected" || code === "transaction_active"
      ? "rejected"
      : "unknown";
  return new ActionError(code, execution, error);
}
