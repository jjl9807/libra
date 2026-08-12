/** Domain-local mirror of W2-12 `RuntimeUsageTotals` (not on CodeUiSessionSnapshot). */

export type UsageStatus = "known" | "partial" | "unknown";

export interface RuntimeUsageTotals {
  requestCount: number;
  totalTokens: number;
  costUsd?: number;
  costEstimateMicroDollars?: number;
  usageStatus: UsageStatus;
  costStatus: UsageStatus;
  errorStatus: UsageStatus;
  failedCount: number;
  unknownUsageCount: number;
  unknownCostCount: number;
}

export interface SubAgentUsageRow {
  agentRunId?: string;
  agentName?: string;
  totals: RuntimeUsageTotals;
}

/**
 * Composite read model for the usage panel. Live HTTP lands in W3-01;
 * until then SessionUsage consumes fixtures / injected transport.
 */
export interface UsageReadModel {
  turnId?: string;
  sessionId?: string;
  cumulative: RuntimeUsageTotals;
  turnDelta?: RuntimeUsageTotals;
  /**
   * Sub-agent rows when attribution is available. `undefined` means the live
   * endpoint omitted attribution (`subAgentsStatus: "unavailable"`); `[]` is a
   * known empty set.
   */
  subAgents?: SubAgentUsageRow[];
  /** Present on live `/api/code/usage` when sub-agent attribution is omitted. */
  subAgentsStatus?: "available" | "unavailable";
  /** Event ids already folded into the read model (presentation / dedupe demos). */
  foldedEventIds?: string[];
}
