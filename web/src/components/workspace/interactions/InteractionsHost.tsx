"use client";

import type {
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
} from "../../../lib/code-ui/types";
import { ApprovalPanel } from "./ApprovalPanel";
import { RequestUserInputForm } from "./RequestUserInputForm";

export interface InteractionsHostProps {
  interaction?: CodeUiInteractionRequest;
  onRespond(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancel(): void | Promise<void>;
  respondEnabled?: boolean;
}

export function InteractionsHost({
  interaction,
  onRespond,
  onCancel,
  respondEnabled = true,
}: InteractionsHostProps) {
  if (!interaction || interaction.status !== "pending") return null;
  if (interaction.kind === "approval" || interaction.kind === "sandbox_approval") {
    return (
      <ApprovalPanel
        key={interaction.id}
        interaction={interaction}
        onRespond={onRespond}
        onCancel={onCancel}
        respondEnabled={respondEnabled}
      />
    );
  }
  if (interaction.kind === "request_user_input") {
    return (
      <RequestUserInputForm
        key={interaction.id}
        interaction={interaction}
        onRespond={onRespond}
        onCancel={onCancel}
        respondEnabled={respondEnabled}
      />
    );
  }
  return null;
}

export { ApprovalPanel } from "./ApprovalPanel";
export { RequestUserInputForm } from "./RequestUserInputForm";
export { SessionInteractions } from "./SessionInteractions";
