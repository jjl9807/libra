import type { RuntimeUsageTotals, UsageReadModel } from "./types";

export function totalsFixture(
  overrides: Partial<RuntimeUsageTotals> = {},
): RuntimeUsageTotals {
  return {
    requestCount: 1,
    totalTokens: 10,
    costUsd: 0.01,
    usageStatus: "known",
    costStatus: "known",
    errorStatus: "known",
    failedCount: 0,
    unknownUsageCount: 0,
    unknownCostCount: 0,
    ...overrides,
  };
}

/** Mirrors `code_usage_attribution_runtime_service` cumulative after parent+child+failed. */
export function partialCumulativeFixture(): RuntimeUsageTotals {
  return totalsFixture({
    requestCount: 3,
    totalTokens: 10,
    costUsd: undefined,
    usageStatus: "partial",
    costStatus: "unknown",
    errorStatus: "partial",
    failedCount: 1,
    unknownUsageCount: 1,
    unknownCostCount: 3,
  });
}

export function unknownSubAgentFixture(): RuntimeUsageTotals {
  return totalsFixture({
    requestCount: 1,
    totalTokens: 0,
    costUsd: undefined,
    usageStatus: "unknown",
    costStatus: "unknown",
    errorStatus: "known",
    failedCount: 0,
    unknownUsageCount: 1,
    unknownCostCount: 1,
  });
}

export function turnDeltaFixture(): RuntimeUsageTotals {
  return totalsFixture({
    requestCount: 2,
    totalTokens: 10,
    costUsd: undefined,
    usageStatus: "partial",
    costStatus: "unknown",
    errorStatus: "known",
    failedCount: 0,
    unknownUsageCount: 1,
    unknownCostCount: 2,
  });
}

export function usageReadModelFixture(
  overrides: Partial<UsageReadModel> = {},
): UsageReadModel {
  return {
    turnId: "turn-parent",
    sessionId: "session-fixture",
    cumulative: partialCumulativeFixture(),
    turnDelta: turnDeltaFixture(),
    subAgents: [
      {
        agentRunId: "child-run",
        agentName: "reviewer",
        totals: unknownSubAgentFixture(),
      },
    ],
    foldedEventIds: ["parent-event", "child-event", "failed-event"],
    ...overrides,
  };
}

/**
 * Apply a usage update event into a read model without re-aggregating cost.
 * Duplicate `eventId` values are ignored (same semantics as RuntimeUsageService
 * event_id fold) — the panel only replaces with the supplied totals snapshot.
 */
export function applyUsageUpdate(
  current: UsageReadModel,
  update: {
    eventId: string;
    next: UsageReadModel;
  },
): UsageReadModel {
  const seen = new Set(current.foldedEventIds ?? []);
  if (seen.has(update.eventId)) {
    return current;
  }
  return {
    ...update.next,
    foldedEventIds: [...seen, update.eventId],
  };
}
