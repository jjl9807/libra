"use client";

import type { ExecutionRepairView } from "../../../lib/code-ui/execution-repair";
import type { CodeUiInteractionResponse } from "../../../lib/code-ui/types";

import { ExecutionProgressPanel } from "./ExecutionProgressPanel";
import { RepairPanel } from "./RepairPanel";

export interface ExecutionRepairHostProps {
  view: ExecutionRepairView;
  busy?: boolean;
  error?: string;
  canRespond?: boolean;
  onContinue(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancelRepair(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
}

export function ExecutionRepairHost({
  view,
  busy,
  error,
  canRespond,
  onContinue,
  onCancelRepair,
}: ExecutionRepairHostProps) {
  return (
    <div aria-label="Execution repair workspace">
      <ExecutionProgressPanel plans={view.plans} toolCalls={view.toolCalls} />
      <RepairPanel
        repair={view.repair}
        interactionId={view.pendingRepairInteraction?.id ?? view.repair?.interaction_id}
        busy={busy}
        error={error}
        canRespond={canRespond}
        onContinue={onContinue}
        onCancel={onCancelRepair}
      />
    </div>
  );
}

export { ExecutionProgressPanel } from "./ExecutionProgressPanel";
export { RepairPanel } from "./RepairPanel";
