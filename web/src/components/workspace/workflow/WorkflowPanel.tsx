"use client";

import { useState } from "react";

import {
  buildWorkflowResponse,
  type WorkflowKind,
} from "../../../lib/code-ui/workflow";
import type {
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
  CodeUiPlanSnapshot,
} from "../../../lib/code-ui/types";

export interface WorkflowPanelProps {
  kind: WorkflowKind;
  interaction: CodeUiInteractionRequest;
  plans?: CodeUiPlanSnapshot[];
  networkAccess?: boolean;
  respondEnabled?: boolean;
  cancelEnabled?: boolean;
  busy?: boolean;
  error?: string;
  onRespond(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancelTurn(): void | Promise<void>;
}

function heading(kind: WorkflowKind): string {
  switch (kind) {
    case "intent_review":
      return "IntentSpec review";
    case "plan_review":
      return "Plan review";
    case "network_policy":
      return "Network policy";
  }
}

export function WorkflowPanel({
  kind,
  interaction,
  plans = [],
  networkAccess,
  respondEnabled = true,
  cancelEnabled = true,
  busy = false,
  error,
  onRespond,
  onCancelTurn,
}: WorkflowPanelProps) {
  const [localError, setLocalError] = useState<string>();

  const run = async (operation: () => Promise<void>) => {
    if (busy) return;
    setLocalError(undefined);
    try {
      await operation();
    } catch (cause) {
      setLocalError(
        cause instanceof Error ? cause.message : "Could not deliver this workflow choice.",
      );
    }
  };

  return (
    <section aria-label={`${heading(kind)} panel`}>
      <h2>{interaction.title ?? heading(kind)}</h2>
      {interaction.description ? <p>{interaction.description}</p> : null}
      {interaction.prompt ? <pre>{interaction.prompt}</pre> : null}
      {kind === "plan_review" || kind === "network_policy" ? (
        <div>
          {plans.map((plan) => (
            <div key={plan.id}>
              <p>
                Plan {plan.id}
                {plan.title ? `: ${plan.title}` : ""}
              </p>
              <ul>
                {plan.steps.map((step) => (
                  <li key={step.step}>
                    {step.step} ({step.status})
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      ) : null}
      {kind === "network_policy" ? (
        <p role="status">
          Requested network access: {networkAccess ? "enabled" : "disabled"} (explicit choice
          required; defaults are not applied automatically).
        </p>
      ) : null}
      {!respondEnabled ? (
        <p role="status">
          This managed Codex session cannot resolve workflow choices from the browser. Resolve them
          in the Codex client, or cancel the turn here.
        </p>
      ) : null}
      <div>
        {interaction.options.map((option) => (
          <button
            key={option.id}
            type="button"
            disabled={busy || !respondEnabled}
            onClick={() =>
              void run(async () => {
                await onRespond(interaction.id, buildWorkflowResponse(kind, option.id));
              })
            }
          >
            {option.label}
          </button>
        ))}
      </div>
      <button
        type="button"
        disabled={busy || !cancelEnabled}
        onClick={() => void run(async () => onCancelTurn())}
      >
        Cancel turn
      </button>
      {localError || error ? <p role="alert">{localError ?? error}</p> : null}
    </section>
  );
}
