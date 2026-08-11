import { describe, expect, it } from "vitest";

import {
  awaitingRepairSessionFixture,
  canContinueRepair,
  continueMaxAttempts,
  executionRepairView,
  exhaustedRepairSessionFixture,
  manualActionRepairFixture,
} from ".";
import { repairFixture, sessionFixture } from "../fixtures";

describe("execution-repair helpers", () => {
  it("projects plans, tools, and repair without inventing state", () => {
    const view = executionRepairView(awaitingRepairSessionFixture());
    expect(view.plans).toHaveLength(1);
    expect(view.repair?.state).toBe("awaiting_user");
    expect(view.pendingRepairInteraction?.kind).toBe("plan_execution_repair");
    expect(executionRepairView(sessionFixture()).repair).toBeUndefined();
  });

  it("only raises maxAttempts when the projected budget is exhausted", () => {
    expect(continueMaxAttempts(repairFixture())).toBeUndefined();
    expect(canContinueRepair(repairFixture())).toBe(true);
    expect(
      continueMaxAttempts(
        repairFixture({
          evidence: {
            output: "failed",
            diagnostics: [],
            attempt: 2,
            max_attempts: 2,
          },
        }),
      ),
    ).toBe(3);
    expect(continueMaxAttempts(manualActionRepairFixture())).toBeUndefined();
    expect(
      continueMaxAttempts(
        repairFixture({
          evidence: {
            output: "failed",
            diagnostics: [],
            attempt: 10,
            max_attempts: 10,
          },
        }),
      ),
    ).toBeUndefined();
    expect(
      canContinueRepair(
        repairFixture({
          evidence: {
            output: "failed",
            diagnostics: [],
            attempt: 10,
            max_attempts: 10,
          },
        }),
      ),
    ).toBe(false);
  });

  it("keeps exhausted repair evidence visible as projected", () => {
    const view = executionRepairView(exhaustedRepairSessionFixture());
    expect(view.repair?.evidence.attempt).toBe(2);
    expect(view.repair?.evidence.max_attempts).toBe(2);
    expect(continueMaxAttempts(view.repair!)).toBe(3);
  });
});
