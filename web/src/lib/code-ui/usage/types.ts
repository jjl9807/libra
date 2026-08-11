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
 * Composite read model for the usage panel.
 */
export interface UsageReadModel {
  turnId?: string;
  sessionId?: string;
  cumulative: RuntimeUsageTotals;
  turnDelta?: RuntimeUsageTotals;
  /**
   * Absent when durable per-sub-agent enumeration is unavailable. An empty
   * array would falsely mean "known zero sub-agents".
   */
  subAgents?: SubAgentUsageRow[];
  /** `unavailable` until RuntimeUsageService can enumerate child agents. */
  subAgentsStatus?: "unavailable" | "ready";
  /** Event ids already folded into the read model (presentation / dedupe demos). */
  foldedEventIds?: string[];
}
