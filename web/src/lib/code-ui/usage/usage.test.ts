import { describe, expect, it } from "vitest";

import {
  applyUsageUpdate,
  createUsageApi,
  formatCost,
  formatTokens,
  isIncompleteUsage,
  partialCumulativeFixture,
  totalsFixture,
  unknownSubAgentFixture,
  usageReadModelFixture,
  usageStatus,
} from ".";

describe("usage helpers", () => {
  it("formats known totals without inventing price", () => {
    const known = totalsFixture({ costUsd: 0.0123, costEstimateMicroDollars: undefined });
    expect(formatCost(known)).toBe("$0.0123");
    expect(formatTokens(known)).toBe("10");
    expect(isIncompleteUsage(known)).toBe(false);
  });

  it("keeps partial/unknown visible instead of pretending zero spend", () => {
    const partial = partialCumulativeFixture();
    expect(formatTokens(partial)).toContain("partial");
    expect(formatTokens(partial)).toContain("unknown_usage=1");
    expect(formatCost(partial)).toContain("unknown");
    expect(isIncompleteUsage(partial)).toBe(true);

    const unknown = unknownSubAgentFixture();
    expect(unknown.totalTokens).toBe(0);
    expect(formatTokens(unknown)).toMatch(/0 \(unknown; unknown_usage=1\)/);
    expect(usageStatus(1, 1)).toBe("unknown");
  });

  it("ignores duplicate event ids when applying updates", () => {
    const base = usageReadModelFixture();
    const inflated = usageReadModelFixture({
      cumulative: totalsFixture({ requestCount: 99, totalTokens: 999 }),
    });
    const afterReplay = applyUsageUpdate(base, {
      eventId: "parent-event",
      next: inflated,
    });
    expect(afterReplay.cumulative.requestCount).toBe(3);
    expect(afterReplay.cumulative.totalTokens).toBe(10);

    const afterNew = applyUsageUpdate(base, {
      eventId: "new-event",
      next: inflated,
    });
    expect(afterNew.cumulative.requestCount).toBe(99);
    expect(afterNew.foldedEventIds).toContain("new-event");
  });

  it("keeps sub-agent rows distinct from cumulative", () => {
    const model = usageReadModelFixture();
    expect(model.subAgents?.[0]?.totals.usageStatus).toBe("unknown");
    expect(model.cumulative.usageStatus).toBe("partial");
    expect(model.subAgents?.[0]?.agentName).toBe("reviewer");
  });

  it("lists usage through the domain API", async () => {
    const response = usageReadModelFixture();
    const api = createUsageApi({
      async request<T>(path: string): Promise<T> {
        expect(path).toBe(
          "/api/code/usage?sessionId=session-fixture&threadId=thread-fixture&turnId=turn-parent",
        );
        return response as T;
      },
    });
    await expect(
      api.fetchReadModel({
        sessionId: "session-fixture",
        threadId: "thread-fixture",
        turnId: "turn-parent",
      }),
    ).resolves.toEqual(response);
  });
});
