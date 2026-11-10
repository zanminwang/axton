// Named Mutation submission, shared by the Node and React Native
// transactions: it imports nothing platform-specific.
import type { RecordValue } from "./values.mts";
import { CallError, type Call } from "./actions.mts";

/**
 * The host's `submitMutation`: the runtime's answer registers a provisional
 * Call while it is dispatched.
 */
export type MutationPort = {
  submit<T>(
    command: RecordValue,
    scope: string | undefined,
    decode: (value: unknown) => T,
  ): Promise<Call<T>>;
};

/** What a transaction adapter lends the shared Mutation submission. */
export type SubmissionHost = {
  /** The adapter's refusal of an outer command, if any. */
  admit(): Promise<never> | undefined;
  /** Track the submission as one of the transaction's commands. */
  track(submit: (scope: string | undefined) => Promise<any>): Promise<any>;
  mutations: MutationPort | undefined;
};

/**
 * Queue a named Mutation in `host`'s transaction. It settles after the
 * Mutation's optimism ran, with a Call that stays provisional until the
 * transaction commits. The input is an object; a caller that reads first
 * does so in the same transaction.
 */
export function submitMutation<T>(
  host: SubmissionHost,
  name: string,
  version: number,
  input: object,
  decode: (value: unknown) => T,
): Promise<Call<T>> {
  if (input === null || typeof input !== "object")
    return Promise.reject(new CallError("action.invalid", "rejected"));
  const refused = host.admit();
  if (refused) return refused;
  const mutations = host.mutations;
  return host.track((scope) => {
    if (!mutations) throw Error("transaction cannot submit a Mutation");
    return mutations.submit(
      { kind: "submitMutation", name, version, args: input },
      scope,
      decode,
    );
  });
}
