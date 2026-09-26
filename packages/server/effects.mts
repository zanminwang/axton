/**
 * The declarations one handler, legacy handler or `backend.transaction` body
 * makes while it runs: changed records (`touch`) and Channel membership
 * intents (`channel(name).todo.add/remove`, `channel(name).add/remove`).
 *
 * Declarations are synchronous and owned: each call validates its identity
 * against the Model's identity fields and copies those fields at once, so a
 * later change to the caller's object, Date or array cannot retarget it. A
 * collector closes when its callback settles; every later declaration,
 * through any escaped handle, is refused. The Rust engine owns what the
 * declarations mean: it infers input targets, reduces membership intents to
 * their final state and settles them.
 */
import type {
  HostRecordRef,
  MembershipIntent,
  SettlementEffects,
} from "./host-contract.mts";

/**
 * A record named by Model and identity. Generated backends narrow it to a
 * union of each Model with its own identity type and generate a constructor
 * per Model, e.g. `Todo({ id })`.
 */
export interface RecordRef {
  readonly model: string;
  readonly identity: object;
}
/** One Model's membership writer on a Channel: `channel(name).todo`. */
export interface RuntimeModelMembership {
  add(identity: object): void;
  remove(identity: object): void;
}
/**
 * A Channel handle: one membership writer per Model under its lower-first
 * accessor, plus `add` and `remove` for mixed lists of record references.
 */
export type RuntimeChannel = {
  readonly [model: string]: RuntimeModelMembership;
} & {
  add(records: readonly RecordRef[]): void;
  remove(records: readonly RecordRef[]): void;
};
/** One change declaration per Model under its lower-first accessor: `touch.todo(identity)`. */
export type RuntimeTouch = {
  readonly [model: string]: (identity: object) => void;
};
export interface EffectCollector {
  readonly touch: RuntimeTouch;
  /** Selects a Channel by name. Creates nothing: the name is only validated. */
  channel(name: string): RuntimeChannel;
  /** A legacy slot operand the change set starts with. Never handed to application code. */
  seed(record: RecordRef): void;
  /** Owned copies of the declarations, readable after `close`. */
  settlement(): SettlementEffects;
  /** Refuses every later declaration, through any handle. Idempotent. */
  close(): void;
}
/** A configured Model descriptor, as the compiled schema's `models` holds it. */
export type EffectModel = {
  name: string;
  identity?: readonly string[];
  fields?: readonly { name: string; type?: unknown }[];
};

export function lowerFirst(name: string): string {
  return name.charAt(0).toLowerCase() + name.slice(1);
}

/** A copied, encoded identity: plain own properties in identity order, frozen. */
type Identity = Readonly<Record<string, unknown>>;
type Entry = {
  name: string;
  key: string;
  snapshot(value: unknown, caller: string): Identity;
};
const INVALID = Symbol("invalid");

/** Encodes one identity component as the engine receives it, or answers INVALID. */
function component(
  model: string,
  field: string,
  type: unknown,
): [expected: string, encode: (value: unknown) => unknown] {
  const { kind, name } = (type ?? {}) as { kind?: unknown; name?: unknown };
  if (kind === "enum")
    return ["an enum value", (v) => (typeof v === "string" ? v : INVALID)];
  if (kind === "scalar")
    switch (name) {
      case "string":
      case "uuid":
        return ["a string", (v) => (typeof v === "string" ? v : INVALID)];
      case "boolean":
        return ["a boolean", (v) => (typeof v === "boolean" ? v : INVALID)];
      case "int":
        return [
          "a safe integer",
          (v) => (Number.isSafeInteger(v) ? v : INVALID),
        ];
      case "float":
        return [
          "a finite number",
          (v) => (typeof v === "number" && Number.isFinite(v) ? v : INVALID),
        ];
      case "dateTime":
        // A decoded Date is encoded now; a wire string (a legacy slot
        // identity) passes through for the engine to canonicalize.
        return [
          "a valid Date or an ISO date-time string",
          (v) =>
            v instanceof Date
              ? Number.isNaN(v.getTime())
                ? INVALID
                : v.toISOString()
              : typeof v === "string" &&
                  /^\d{4}-\d{2}-\d{2}T/.test(v) &&
                  !Number.isNaN(Date.parse(v))
                ? v
                : INVALID,
        ];
    }
  throw new Error(
    `${model} identity field ${field} has an unsupported type ${JSON.stringify(type)}`,
  );
}

function define(target: object, key: string, value: unknown): void {
  Object.defineProperty(target, key, { value, enumerable: true });
}

/**
 * Validates the configured Models once: every Model needs identity fields
 * with supported types, and its lower-first accessor must be unique and
 * neither `add` nor `remove`, which a Channel reserves for mixed lists.
 */
function entriesOf(models: readonly EffectModel[]): Entry[] {
  const owners = new Map<string, string>();
  return models.map((model) => {
    const name = model?.name;
    if (typeof name !== "string" || name === "")
      throw new Error("every Model descriptor needs a name");
    const key = lowerFirst(name);
    if (key === "add" || key === "remove")
      throw new Error(
        `Model ${name} generates the accessor ${key}, which a Channel reserves for mixed record lists; rename the Model`,
      );
    const other = owners.get(key);
    if (other !== undefined)
      throw new Error(
        `Models ${other} and ${name} both generate the accessor ${key}; rename one`,
      );
    owners.set(key, name);
    if (!Array.isArray(model.identity) || model.identity.length === 0)
      throw new Error(`Model ${name} declares no identity fields`);
    const components = model.identity.map((field: string) => {
      const declared = model.fields?.find(
        (candidate) => candidate.name === field,
      );
      if (!declared)
        throw new Error(
          `${name} identity field ${field} is not one of its fields`,
        );
      return [field, ...component(name, field, declared.type)] as const;
    });
    const snapshot = (value: unknown, caller: string): Identity => {
      if (value === null || typeof value !== "object")
        throw new Error(`${caller}: ${name} identity must be an object`);
      const copy = {};
      for (const [field, expected, encode] of components) {
        const raw = (value as Record<string, unknown>)[field];
        if (raw === undefined || raw === null)
          throw new Error(
            `${caller}: ${name} identity field ${field} is missing`,
          );
        const encoded = encode(raw);
        if (encoded === INVALID)
          throw new Error(
            `${caller}: ${name} identity field ${field} must be ${expected}`,
          );
        define(copy, field, encoded);
      }
      return Object.freeze(copy);
    };
    return { name, key, snapshot };
  });
}

/**
 * Validates `models` once and answers a factory of callback-scoped
 * collectors. A malformed configuration throws here, at startup.
 */
export function effectsFor(
  models: readonly EffectModel[],
): () => EffectCollector {
  const entries = entriesOf(models);
  const byName = new Map(entries.map((entry) => [entry.name, entry]));
  /** Resolves one explicit reference; a raw identity names no Model and fails. */
  const reference = (
    value: unknown,
    caller: string,
  ): { model: string; identity: Identity } => {
    const model = (value as { model?: unknown } | null)?.model;
    if (
      value === null ||
      typeof value !== "object" ||
      typeof model !== "string"
    )
      throw new Error(
        `${caller}: each element must be a record reference such as Todo({ id }); a raw identity names no Model`,
      );
    const entry = byName.get(model);
    if (!entry) throw new Error(`${caller}: unknown Model ${model}`);
    return {
      model,
      identity: entry.snapshot((value as RecordRef).identity, caller),
    };
  };
  return () => {
    let open = true;
    const changes: HostRecordRef[] = [];
    const changed = new Set<string>();
    const memberships: MembershipIntent[] = [];
    const assertOpen = (caller: string) => {
      if (!open)
        throw new Error(
          `${caller}: the callback has settled and its declarations are closed`,
        );
    };
    const change = (model: string, identity: Identity) => {
      const key = `${model}\u0000${JSON.stringify(Object.values(identity))}`;
      if (changed.has(key)) return;
      changed.add(key);
      changes.push(Object.freeze({ model, identity }) as HostRecordRef);
    };
    const intent = (
      channel: string,
      model: string,
      identity: Identity,
      present: boolean,
    ) =>
      memberships.push(
        Object.freeze({
          channel,
          model,
          identity,
          present,
        }) as MembershipIntent,
      );
    const touch = Object.create(null);
    for (const entry of entries)
      define(touch, entry.key, (identity: object) => {
        const caller = `touch.${entry.key}`;
        assertOpen(caller);
        change(entry.name, entry.snapshot(identity, caller));
      });
    Object.freeze(touch);
    const channel = (name: string): RuntimeChannel => {
      assertOpen("channel");
      if (typeof name !== "string" || name.trim() === "")
        throw new Error("channel: a Channel name must be a nonblank string");
      const label = `channel(${JSON.stringify(name)})`;
      const handle = Object.create(null);
      for (const entry of entries) {
        const membership = Object.create(null);
        for (const [verb, present] of [
          ["add", true],
          ["remove", false],
        ] as const)
          define(membership, verb, (identity: object) => {
            const caller = `${label}.${entry.key}.${verb}`;
            assertOpen(caller);
            intent(name, entry.name, entry.snapshot(identity, caller), present);
          });
        define(handle, entry.key, Object.freeze(membership));
      }
      for (const [verb, present] of [
        ["add", true],
        ["remove", false],
      ] as const)
        define(handle, verb, (records: readonly RecordRef[]) => {
          const caller = `${label}.${verb}`;
          assertOpen(caller);
          if (!Array.isArray(records))
            throw new Error(
              `${caller}: expected an array of record references`,
            );
          // Every element is resolved before any is declared, so a caught
          // failure leaves no partial declaration behind.
          const resolved = [];
          for (let index = 0; index < records.length; index++)
            resolved.push(reference(records[index], caller));
          for (const { model, identity } of resolved)
            intent(name, model, identity, present);
        });
      return Object.freeze(handle) as RuntimeChannel;
    };
    return Object.freeze({
      touch: touch as RuntimeTouch,
      channel,
      seed(record: RecordRef) {
        assertOpen("seed");
        const { model, identity } = reference(record, "seed");
        change(model, identity);
      },
      settlement: (): SettlementEffects => ({
        changes: [...changes],
        memberships: [...memberships],
      }),
      close() {
        open = false;
      },
    });
  };
}

/** One collector over `models`, validating them first. */
export function createEffects(models: readonly EffectModel[]): EffectCollector {
  return effectsFor(models)();
}
