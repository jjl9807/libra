import { interactionFixture, sessionFixture } from "../fixtures";
import type { CodeUiInteractionRequest, CodeUiSessionSnapshot } from "../types";

const INTENT_OPTIONS = [
  { id: "confirm", label: "Confirm" },
  { id: "modify", label: "Modify" },
  { id: "cancel", label: "Cancel" },
];

const PLAN_OPTIONS = [
  { id: "execute", label: "Execute" },
  { id: "modify", label: "Modify" },
  { id: "cancel", label: "Cancel" },
];

const NETWORK_OPTIONS = [
  { id: "network-deny", label: "Deny network" },
  { id: "network-allow", label: "Allow network" },
  { id: "back", label: "Back" },
];

export function intentReviewInteraction(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return interactionFixture({
    id: "intent-review-1",
    kind: "intent_review_choice",
    title: "IntentSpec review",
    description: "Confirm, modify, or cancel this IntentSpec.",
    options: INTENT_OPTIONS,
    metadata: {},
    ...overrides,
  });
}

export function planReviewInteraction(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return interactionFixture({
    id: "plan-review-1",
    kind: "post_plan_choice",
    title: "Plan review",
    description: "Execute, modify, or cancel this plan.",
    options: PLAN_OPTIONS,
    metadata: {
      intentId: "intent-1",
      planId: "plan-1",
      networkAccess: false,
    },
    ...overrides,
  });
}

export function networkPolicyInteraction(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return interactionFixture({
    id: "plan-1:network-policy",
    kind: "post_plan_choice",
    title: "Network policy",
    description: "Explicitly allow or deny network access for plan execution.",
    options: NETWORK_OPTIONS,
    metadata: {
      intentId: "intent-1",
      planId: "plan-1",
      networkAccess: false,
      phase: "networkPolicy",
    },
    ...overrides,
  });
}

const samplePlan = {
  id: "plan-1",
  title: "Sample plan",
  summary: "Fixture plan for workflow review",
  status: "awaiting_review",
  steps: [
    { step: "Inspect repository", status: "pending" },
    { step: "Apply change", status: "pending" },
  ],
  updatedAt: "2026-07-15T00:00:00.000Z",
};

export function intentReviewSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({
    status: "awaiting_interaction",
    interactions: [intentReviewInteraction()],
    ...overrides,
  });
}

export function planReviewSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({
    status: "awaiting_interaction",
    plans: [samplePlan],
    interactions: [planReviewInteraction()],
    ...overrides,
  });
}

export function networkPolicySessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({
    status: "awaiting_interaction",
    plans: [samplePlan],
    interactions: [networkPolicyInteraction()],
    ...overrides,
  });
}
