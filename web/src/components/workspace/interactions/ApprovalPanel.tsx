"use client";

import { useState } from "react";

import {
  buildApprovalResponse,
  isApprovalKind,
  readApprovalMetadata,
} from "../../../lib/code-ui/interactions";
import type {
  CodeUiApplyToFuture,
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
} from "../../../lib/code-ui/types";

export interface ApprovalPanelProps {
  interaction: CodeUiInteractionRequest;
  onRespond(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancel(): void | Promise<void>;
  /** When false, show the request read-only (managed Codex cannot resolve posts). */
  respondEnabled?: boolean;
}

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Could not deliver this approval. Try again.";
}

export function ApprovalPanel({
  interaction,
  onRespond,
  onCancel,
  respondEnabled = true,
}: ApprovalPanelProps) {
  const [applyToFuture, setApplyToFuture] = useState<CodeUiApplyToFuture>("no");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const metadata = readApprovalMetadata(interaction.metadata);

  if (!isApprovalKind(interaction)) return null;

  const run = async (operation: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    try {
      await operation();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-label="Approval request">
      <p>Interaction: {interaction.id}</p>
      <p>Kind: {interaction.kind}</p>
      {interaction.title && <h2>{interaction.title}</h2>}
      {interaction.description && <p>{interaction.description}</p>}
      {interaction.prompt && <pre>{interaction.prompt}</pre>}
      {metadata.command && <p>Command: {metadata.command}</p>}
      {metadata.cwd && <p>Working directory: {metadata.cwd}</p>}
      {metadata.reason && <p>Reason: {metadata.reason}</p>}
      {metadata.sandboxLabel && <p>Sandbox: {metadata.sandboxLabel}</p>}
      {!respondEnabled && (
        <p role="status">
          This managed Codex session cannot resolve approvals from the browser. Approve or deny them
          in the Codex client, or cancel the turn here.
        </p>
      )}

      <label>
        Apply to future commands
        <select
          aria-label="Apply to future commands"
          value={applyToFuture}
          disabled={busy || !respondEnabled}
          onChange={(event) => setApplyToFuture(event.target.value as CodeUiApplyToFuture)}
        >
          <option value="no">No</option>
          <option value="accept_all">Accept all</option>
          <option value="decline_all">Decline all</option>
        </select>
      </label>

      <div>
        {interaction.options.map((option) => (
          <button
            key={option.id}
            type="button"
            disabled={busy || !respondEnabled}
            onClick={() =>
              void run(async () => {
                await onRespond(
                  interaction.id,
                  buildApprovalResponse({
                    selectedOption: option.id as "approve" | "deny" | "abort",
                    applyToFuture,
                  }),
                );
              })
            }
          >
            {option.label}
          </button>
        ))}
        <button type="button" disabled={busy} onClick={() => void run(async () => onCancel())}>
          Cancel
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
