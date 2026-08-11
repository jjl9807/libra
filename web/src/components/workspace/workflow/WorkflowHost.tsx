"use client";

import type { WorkflowView } from "../../../lib/code-ui/workflow";
import type { CodeUiInteractionResponse } from "../../../lib/code-ui/types";

import { WorkflowPanel } from "./WorkflowPanel";

export interface WorkflowHostProps {
  view: WorkflowView;
  respondEnabled?: boolean;
  cancelEnabled?: boolean;
  busy?: boolean;
  error?: string;
  onRespond(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancelTurn(): void | Promise<void>;
}

export function WorkflowHost({
  view,
  respondEnabled = true,
  cancelEnabled = true,
  busy,
  error,
  onRespond,
  onCancelTurn,
}: WorkflowHostProps) {
  if (!view.kind || !view.interaction) return null;
  return (
    <div aria-label="Workflow review workspace">
      <WorkflowPanel
        kind={view.kind}
        interaction={view.interaction}
        plans={view.plans}
        networkAccess={view.networkAccess}
        respondEnabled={respondEnabled}
        cancelEnabled={cancelEnabled}
        busy={busy}
        error={error}
        onRespond={onRespond}
        onCancelTurn={onCancelTurn}
      />
    </div>
  );
}

export { WorkflowPanel } from "./WorkflowPanel";
