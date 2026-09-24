/** Transaction faults which must reach the adapter's whole-transaction retry loop. */
export function isRetryableTransactionError(error: unknown): boolean {
  const seen = new Set<unknown>();
  let current: unknown = error;
  while (current && typeof current === "object" && !seen.has(current)) {
    seen.add(current);
    const value = current as {
      code?: string;
      meta?: { code?: string };
      cause?: unknown;
    };
    if (
      value.code === "40001" ||
      value.code === "40P01" ||
      value.code === "P2034"
    )
      return true;
    if (
      value.code === "P2010" &&
      (value.meta?.code === "40001" || value.meta?.code === "40P01")
    )
      return true;
    current = value.cause;
  }
  return false;
}
