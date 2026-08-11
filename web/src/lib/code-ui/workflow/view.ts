import type {
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
  CodeUiPlanSnapshot,
  CodeUiSessionSnapshot,
  JsonValue,
} from "../types";

export type WorkflowKind = "intent_review" | "plan_review" | "network_policy";

export interface WorkflowView {
  kind?: WorkflowKind;
  interaction?: CodeUiInteractionRequest;
  plans: CodeUiPlanSnapshot[];
  networkAccess?: boolean;
  intentId?: string;
  planId?: string;
}

function isRecord(value: JsonValue): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isNetworkPolicyInteraction(interaction: CodeUiInteractionRequest): boolean {
  if (interaction.kind !== "post_plan_choice") return false;
  if (!isRecord(interaction.metadata)) return false;
  return interaction.metadata.phase === "networkPolicy";
}

export function isIntentReviewInteraction(interaction: CodeUiInteractionRequest): boolean {
  return interaction.kind === "intent_review_choice";
}

export function isPlanReviewInteraction(interaction: CodeUiInteractionRequest): boolean {
  return interaction.kind === "post_plan_choice" && !isNetworkPolicyInteraction(interaction);
}

export function readPlanWorkflowMetadata(metadata: JsonValue): {
  intentId?: string;
  planId?: string;
  networkAccess?: boolean;
} {
  if (!isRecord(metadata)) return {};
  return {
    intentId: typeof metadata.intentId === "string" ? metadata.intentId : undefined,
    planId: typeof metadata.planId === "string" ? metadata.planId : undefined,
    networkAccess:
      typeof metadata.networkAccess === "boolean" ? metadata.networkAccess : undefined,
  };
}

/** Select the first pending workflow interaction from the live snapshot. */
export function workflowView(snapshot?: CodeUiSessionSnapshot): WorkflowView {
  if (!snapshot) return { plans: [] };
  const pending = snapshot.interactions.find(
    (interaction) =>
      interaction.status === "pending" &&
      (isIntentReviewInteraction(interaction) ||
        isPlanReviewInteraction(interaction) ||
        isNetworkPolicyInteraction(interaction)),
  );
  if (!pending) return { plans: snapshot.plans };

  const meta = readPlanWorkflowMetadata(pending.metadata);
  if (isIntentReviewInteraction(pending)) {
    return { kind: "intent_review", interaction: pending, plans: snapshot.plans, ...meta };
  }
  if (isNetworkPolicyInteraction(pending)) {
    return { kind: "network_policy", interaction: pending, plans: snapshot.plans, ...meta };
  }
  return { kind: "plan_review", interaction: pending, plans: snapshot.plans, ...meta };
}

const INTENT_OPTIONS = new Set(["confirm", "modify", "cancel"]);
const PLAN_OPTIONS = new Set(["execute", "modify", "cancel"]);
const NETWORK_OPTIONS = new Set(["network-deny", "network-allow", "back"]);

export function validateWorkflowOption(
  kind: WorkflowKind,
  selectedOption: string,
): string | undefined {
  const trimmed = selectedOption.trim();
  if (!trimmed) return "Select a workflow option before continuing.";
  const allowed =
    kind === "intent_review"
      ? INTENT_OPTIONS
      : kind === "plan_review"
        ? PLAN_OPTIONS
        : NETWORK_OPTIONS;
  if (!allowed.has(trimmed)) {
    return `Unsupported workflow option: ${trimmed}`;
  }
  return undefined;
}

export function buildWorkflowResponse(
  kind: WorkflowKind,
  selectedOption: string,
): CodeUiInteractionResponse {
  const error = validateWorkflowOption(kind, selectedOption);
  if (error) throw new Error(error);
  return { selectedOption: selectedOption.trim(), answers: {} };
}
