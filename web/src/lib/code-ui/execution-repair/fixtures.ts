import { executionFixture, repairFixture, sessionFixture } from "../fixtures";
import type {
  CodeUiInteractionRequest,
  CodeUiPlanExecutionRepair,
  CodeUiSessionSnapshot,
} from "../types";

const now = "2026-07-15T00:00:00.000Z";

export function repairInteractionFixture(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return {
    id: "repair-interaction",
    kind: "plan_execution_repair",
    title: "Plan repair",
    prompt: "Continue automatic repair or cancel?",
    options: [
      { id: "continue", label: "Continue" },
      { id: "cancel", label: "Cancel" },
    ],
    status: "pending",
    metadata: {},
    requestedAt: now,
    ...overrides,
  };
}

export function awaitingRepairSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
  repairOverrides: Partial<CodeUiPlanExecutionRepair> = {},
): CodeUiSessionSnapshot {
  const repair = repairFixture(repairOverrides);
  return sessionFixture({
    status: "awaiting_interaction",
    plans: [
      {
        id: "plan-1",
        title: "Ship fix",
        status: "failed",
        steps: [
          { step: "Run tests", status: "failed" },
          { step: "Apply patch", status: "pending" },
        ],
        updatedAt: now,
      },
    ],
    planExecutionRepair: repair,
    interactions: [repairInteractionFixture({ id: repair.interaction_id ?? "repair-interaction" })],
    controller: { kind: "browser", canWrite: true, loopbackOnly: true },
    ...overrides,
  });
}

export function exhaustedRepairSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return awaitingRepairSessionFixture(overrides, {
    evidence: {
      output: "verification failed again",
      diagnostics: ["flaky test", "timeout"],
      attempt: 2,
      max_attempts: 2,
    },
  });
}

export function automaticRepairSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return executionFixture({
    planExecutionRepair: repairFixture({
      state: "automatic_repair",
      interaction_id: undefined,
      evidence: {
        output: "retrying verification",
        diagnostics: ["attempting auto repair"],
        attempt: 1,
        max_attempts: 3,
      },
    }),
    interactions: [],
    ...overrides,
  });
}

export function manualActionRepairFixture(
  overrides: Partial<CodeUiPlanExecutionRepair> = {},
): CodeUiPlanExecutionRepair {
  return repairFixture({
    state: "manual_action",
    route: "manual_action",
    evidence: {
      output: "needs human patch",
      diagnostics: ["unrecoverable tool failure"],
      attempt: 1,
      max_attempts: 2,
    },
    ...overrides,
  });
}
