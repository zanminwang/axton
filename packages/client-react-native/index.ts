import { requireNativeModule } from "expo-modules-core";
import { createClient } from "../client-js/runtime.mts";
import { Transaction } from "./transaction.mts";
import { createServerConnection } from "./live.mts";
export { Transaction } from "./transaction.mts";
export {
  ActionError,
  type ActionCall,
  type ActionOutcome,
  type ActionStatus,
} from "../client-js/actions.mts";
export type { QuerySpec, RecordValue } from "../client-js/values.mts";
export type {
  Connection,
  ConnectionOptions,
} from "../client-js/connection.mts";
export type { ServerOptions } from "../client-js/live.mts";
export type {
  Subscription,
  SubscriptionState,
  SubscriptionStatus,
} from "../client-js/runtime.mts";
export type {
  ClientSyncState,
  ModelSyncState,
  PendingMutation,
  RebuildReport,
  Rejection,
  SchemaState,
} from "../client-js/runtime.mts";

const native = requireNativeModule<{
  clientCall(request: string): Promise<string>;
  databasePath(name: string): Promise<string>;
}>("AxtonNative");

/** Resolves a basename inside persistent application storage. */
export function databasePath(name = "axton.sqlite"): Promise<string> {
  return native.databasePath(name);
}
export const Client = createClient(native, Transaction, createServerConnection);
export type Client = Awaited<ReturnType<typeof Client.open>>;
