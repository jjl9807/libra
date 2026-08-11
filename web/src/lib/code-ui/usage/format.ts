import type { RuntimeUsageTotals, UsageStatus } from "./types";

/** Match Rust `usage_status(total, unknown)` — display only, never invent cost. */
export function usageStatus(total: number, unknown: number): UsageStatus {
  if (total === 0 || unknown === total) return "unknown";
  if (unknown === 0) return "known";
  return "partial";
}

export function formatCost(totals: RuntimeUsageTotals): string {
  const exact =
    typeof totals.costUsd === "number" ? `$${totals.costUsd.toFixed(4)}` : undefined;
  const estimate =
    typeof totals.costEstimateMicroDollars === "number"
      ? `~$${(totals.costEstimateMicroDollars / 1_000_000).toFixed(4)}`
      : undefined;
  let cost: string;
  if (exact && estimate) cost = `${exact} + ${estimate}`;
  else if (exact) cost = exact;
  else if (estimate) cost = estimate;
  else cost = "unknown";

  if (totals.costStatus === "known") return cost;
  return `${cost} (${totals.costStatus}; unknown_cost=${totals.unknownCostCount})`;
}

export function formatTokens(totals: RuntimeUsageTotals): string {
  const tokens = String(totals.totalTokens);
  if (totals.usageStatus === "known") return tokens;
  // Zero tokens with unknown/partial is incomplete data, not "0 spend".
  return `${tokens} (${totals.usageStatus}; unknown_usage=${totals.unknownUsageCount})`;
}

export function formatErrorStatus(totals: RuntimeUsageTotals): string {
  if (totals.errorStatus === "known") {
    return totals.failedCount === 0 ? "none" : `${totals.failedCount} failed`;
  }
  return `${totals.errorStatus}; failed=${totals.failedCount}`;
}

export function isIncompleteUsage(totals: RuntimeUsageTotals): boolean {
  return (
    totals.usageStatus !== "known" ||
    totals.costStatus !== "known" ||
    totals.errorStatus !== "known"
  );
}
