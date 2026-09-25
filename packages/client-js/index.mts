import { createRequire } from "node:module";
import { createClient } from "./runtime.mts";
import { Transaction } from "./transaction.mts";
import { createServerConnection } from "./live.mts";
export { Transaction, type QuerySpec } from "./transaction.mts";
export {
  ActionError,
  type ActionCall,
  type ActionOutcome,
  type ActionStatus,
} from "./actions.mts";
export type { RecordValue } from "./values.mts";
export type { Connection, ConnectionOptions } from "./connection.mts";
export {
  AxtonReport,
  type ReportDetails,
  type ReportKind,
} from "./connection.mts";
export type { ServerOptions } from "./live.mts";
export type {
  Subscription,
  SubscriptionState,
  SubscriptionStatus,
} from "./runtime.mts";
export type {
  ClientSyncState,
  ModelSyncState,
  PendingMutation,
  RebuildReport,
  Rejection,
  SchemaState,
} from "./runtime.mts";
const native = createRequire(import.meta.url)(
  "../../bindings/node/axton-node.node",
) as {
  clientCall(request: string): Promise<string>;
};
export const Client = createClient(native, Transaction, createServerConnection);
export type Client = Awaited<ReturnType<typeof Client.open>>;
