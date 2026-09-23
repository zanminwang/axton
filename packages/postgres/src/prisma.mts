import type { DriverOptions, PostgresDriver } from "./driver.mts";
import { RETRYABLE_SQLSTATES, withRetries } from "./driver.mts";
import { persistence } from "./persistence.mts";

/** The part of a Prisma interactive transaction AXTON uses; `Prisma.TransactionClient` satisfies it. */
export interface PrismaTransaction {
  $queryRawUnsafe<T = unknown>(sql: string, ...values: any[]): Promise<T>;
}
/** The part of a Prisma client AXTON uses; `PrismaClient` satisfies it. */
export interface PrismaClientLike<Tx extends PrismaTransaction> {
  $transaction<R>(
    body: (tx: Tx) => Promise<R>,
    options: { isolationLevel: "RepeatableRead"; timeout: number },
  ): Promise<R>;
}

const retryable = (error: unknown): boolean => {
  const e = error as { code?: string; meta?: { code?: string } } | null;
  return (
    e?.code === "P2034" ||
    (e?.code === "P2010" && RETRYABLE_SQLSTATES.has(e.meta?.code ?? ""))
  );
};

/** A driver over a Prisma client. Handlers and loaders receive the interactive transaction client. */
export function prismaDriver<Tx extends PrismaTransaction>(
  client: PrismaClientLike<Tx>,
  options: DriverOptions = {},
): PostgresDriver<Tx> {
  return {
    transaction: (body) =>
      withRetries(
        () =>
          client.$transaction(body, {
            isolationLevel: "RepeatableRead",
            timeout: options.timeout ?? 20000,
          }),
        retryable,
        options.retries ?? 3,
      ),
    // Prisma binds `$n` itself and maps bigint/jsonb columns to JS values.
    query: (tx, sql, params) =>
      tx.$queryRawUnsafe<Record<string, unknown>[]>(sql, ...params),
  };
}

/** `createBackend({ database: prisma(client) })`. */
export function prisma<Tx extends PrismaTransaction>(
  client: PrismaClientLike<Tx>,
  options: DriverOptions = {},
) {
  return persistence(prismaDriver(client, options));
}
