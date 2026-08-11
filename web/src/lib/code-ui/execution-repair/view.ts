import type {
  CodeUiInteractionRequest,
  CodeUiPlanExecutionRepair,
  CodeUiPlanSnapshot,
  CodeUiSessionSnapshot,
  CodeUiToolCallSnapshot,
} from "../types";

/** Presentational view — never reclassifies failures or invents attempt caps. */
export interface ExecutionRepairView {
  plans: CodeUiPlanSnapshot[];
  toolCalls: CodeUiToolCallSnapshot[];
  repair?: CodeUiPlanExecutionRepair;
  pendingRepairInteraction?: CodeUiInteractionRequest;
}

export function executionRepairView(
  snapshot?: CodeUiSessionSnapshot,
): ExecutionRepairView {
  if (!snapshot) {
    return { plans: [], toolCalls: [] };
  }
  const repair = snapshot.planExecutionRepair;
  const pendingRepairInteraction = snapshot.interactions.find(
    (interaction) =>
      interaction.status === "pending" &&
      (interaction.kind === "plan_execution_repair" ||
        interaction.id === repair?.interaction_id),
  );
  return {
    plans: snapshot.plans,
    toolCalls: snapshot.toolCalls,
    repair,
    pendingRepairInteraction,
  };
}

/**
 * Continue payload when the projected attempt budget is already exhausted.
 * Uses only the projected max_attempts — does not invent a new classification.
 * Returns undefined when Continue cannot raise the cap further (hard ceiling 10).
 */
export function continueMaxAttempts(repair: CodeUiPlanExecutionRepair): number | undefined {
  const { attempt, max_attempts: maxAttempts } = repair.evidence;
  if (attempt < maxAttempts) return undefined;
  if (maxAttempts >= 10) return undefined;
  return maxAttempts + 1;
}

/** Whether Continue can still advance under the projected attempt budget. */
export function canContinueRepair(repair: CodeUiPlanExecutionRepair): boolean {
  const { attempt, max_attempts: maxAttempts } = repair.evidence;
  if (attempt < maxAttempts) return true;
  return maxAttempts < 10;
}

export function repairStateLabel(state: CodeUiPlanExecutionRepair["state"]): string {
  switch (state) {
    case "automatic_repair":
      return "Automatic repair";
    case "awaiting_user":
      return "Awaiting user";
    case "intent_spec_revision":
      return "IntentSpec revision";
    case "manual_action":
      return "Manual action";
    case "cancelled":
      return "Cancelled";
  }
}
