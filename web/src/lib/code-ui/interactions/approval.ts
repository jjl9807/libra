import type {
  CodeUiApplyToFuture,
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
  JsonValue,
} from "../types";

export interface ApprovalMetadata {
  command?: string;
  cwd?: string;
  reason?: string;
  sandboxLabel?: string;
}

function objectMetadata(metadata: JsonValue): Record<string, JsonValue> | undefined {
  return metadata !== null && !Array.isArray(metadata) && typeof metadata === "object"
    ? metadata
    : undefined;
}

export function isApprovalKind(interaction: CodeUiInteractionRequest): boolean {
  return interaction.kind === "approval" || interaction.kind === "sandbox_approval";
}

export function readApprovalMetadata(metadata: JsonValue): ApprovalMetadata {
  const value = objectMetadata(metadata);
  if (!value) return {};
  const sandboxLabel =
    typeof value.sandboxLabel === "string"
      ? value.sandboxLabel
      : typeof value.sandbox_label === "string"
        ? value.sandbox_label
        : undefined;
  const command = typeof value.command === "string" ? value.command : undefined;
  const cwd = typeof value.cwd === "string" ? value.cwd : undefined;
  const reason =
    typeof value.reason === "string"
      ? value.reason
      : undefined;
  return { command, cwd, reason, sandboxLabel };
}

export function buildApprovalResponse({
  selectedOption,
  applyToFuture,
}: {
  selectedOption: "approve" | "deny" | "abort";
  applyToFuture: CodeUiApplyToFuture;
}): CodeUiInteractionResponse {
  // Drop contradictory apply-to-future values before posting so the Codex
  // sidecar parser does not reject the response and leave the gate pending.
  const normalizedApplyToFuture = (() => {
    if (selectedOption === "abort") return "no" as const;
    if (selectedOption === "approve" && applyToFuture === "decline_all") return "no" as const;
    if (selectedOption === "deny" && applyToFuture === "accept_all") return "no" as const;
    return applyToFuture;
  })();
  return {
    approved: selectedOption === "approve" ? true : selectedOption === "deny" ? false : undefined,
    applyToFuture: normalizedApplyToFuture,
    selectedOption,
    answers: {},
  };
}
