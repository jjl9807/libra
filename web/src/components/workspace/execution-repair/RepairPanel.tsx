"use client";

import {
  canContinueRepair,
  continueMaxAttempts,
  repairStateLabel,
} from "../../../lib/code-ui/execution-repair";
import type { CodeUiInteractionResponse, CodeUiPlanExecutionRepair } from "../../../lib/code-ui/types";

export interface RepairPanelProps {
  repair?: CodeUiPlanExecutionRepair;
  interactionId?: string;
  busy?: boolean;
  error?: string;
  canRespond?: boolean;
  onContinue(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancel(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
}

export function RepairPanel({
  repair,
  interactionId,
  busy = false,
  error,
  canRespond = true,
  onContinue,
  onCancel,
}: RepairPanelProps) {
  if (!repair) {
    return (
      <section aria-label="Repair panel">
        <h2>Repair</h2>
        <p>No plan-execution repair state projected.</p>
      </section>
    );
  }

  const respondId = interactionId ?? repair.interaction_id;
  const raisedMaxAttempts = continueMaxAttempts(repair);
  const continueAllowed = canContinueRepair(repair);
  const awaiting = repair.state === "awaiting_user" && Boolean(respondId) && canRespond;

  return (
    <section aria-label="Repair panel">
      <h2>Repair</h2>
      <p>
        {repairStateLabel(repair.state)}
        {repair.route ? ` · route ${repair.route}` : ""}
      </p>
      <p>
        Attempt {repair.evidence.attempt} / {repair.evidence.max_attempts}
        {raisedMaxAttempts
          ? ` (Continue will raise maxAttempts to ${raisedMaxAttempts})`
          : ""}
      </p>
      <pre aria-label="Repair failure output">{repair.evidence.output}</pre>
      {repair.evidence.diagnostics.length > 0 ? (
        <ul aria-label="Repair diagnostics">
          {repair.evidence.diagnostics.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      ) : null}
      {repair.state === "manual_action" ? (
        <p role="status">Manual action required — follow the projected guidance before continuing.</p>
      ) : null}
      {awaiting && !continueAllowed ? (
        <p role="status">
          Automatic repair retries are exhausted at the hard cap (10). Cancel repair or revise the
          plan manually.
        </p>
      ) : null}
      {awaiting ? (
        <div>
          {continueAllowed ? (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void onContinue(respondId!, {
                  selectedOption: "continue",
                  ...(raisedMaxAttempts ? { maxAttempts: raisedMaxAttempts } : {}),
                  answers: {},
                })
              }
            >
              Continue repair
            </button>
          ) : null}
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void onCancel(respondId!, {
                selectedOption: "cancel",
                answers: {},
              })
            }
          >
            Cancel repair
          </button>
        </div>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
