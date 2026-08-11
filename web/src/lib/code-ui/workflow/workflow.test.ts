import { describe, expect, it } from "vitest";

import {
  buildWorkflowResponse,
  intentReviewSessionFixture,
  networkPolicySessionFixture,
  planReviewSessionFixture,
  validateWorkflowOption,
  workflowView,
} from ".";

describe("workflow helpers", () => {
  it("projects IntentSpec review from the live snapshot", () => {
    const view = workflowView(intentReviewSessionFixture());
    expect(view.kind).toBe("intent_review");
    expect(view.interaction?.options.map((option) => option.id)).toEqual([
      "confirm",
      "modify",
      "cancel",
    ]);
  });

  it("projects plan review and network policy as distinct gates", () => {
    const plan = workflowView(planReviewSessionFixture());
    expect(plan.kind).toBe("plan_review");
    expect(plan.planId).toBe("plan-1");
    expect(plan.plans).toHaveLength(1);

    const network = workflowView(networkPolicySessionFixture());
    expect(network.kind).toBe("network_policy");
    expect(network.networkAccess).toBe(false);
    expect(network.interaction?.options.map((option) => option.id)).toEqual([
      "network-deny",
      "network-allow",
      "back",
    ]);
  });

  it("validates options and builds selectedOption responses", () => {
    expect(validateWorkflowOption("intent_review", "")).toMatch(/Select/);
    expect(validateWorkflowOption("plan_review", "approve")).toMatch(/Unsupported/);
    expect(buildWorkflowResponse("intent_review", "confirm")).toEqual({
      selectedOption: "confirm",
      answers: {},
    });
    expect(buildWorkflowResponse("network_policy", "network-allow")).toEqual({
      selectedOption: "network-allow",
      answers: {},
    });
    expect(() => buildWorkflowResponse("plan_review", "nope")).toThrow(/Unsupported/);
  });

  it("hides resolved workflow interactions", () => {
    const view = workflowView(
      intentReviewSessionFixture({
        interactions: [
          intentReviewSessionFixture().interactions[0]!,
        ].map((interaction) => ({ ...interaction, status: "resolved" as const })),
      }),
    );
    expect(view.kind).toBeUndefined();
    expect(view.interaction).toBeUndefined();
  });
});
