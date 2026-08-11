import { interactionFixture, sessionFixture } from "../fixtures";
import type {
  CodeUiApplyToFuture,
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
  CodeUiSessionSnapshot,
} from "../types";

const approvalOptions = [
  { id: "approve", label: "Approve", description: "Allow this command once" },
  { id: "deny", label: "Deny", description: "Skip this command" },
  { id: "abort", label: "Abort", description: "Cancel this tool run immediately" },
];

export function toolApprovalFixture(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return interactionFixture({
    id: "tool-approval-fixture",
    kind: "approval",
    title: "Command approval required",
    description: "The agent needs permission to run this command.",
    prompt: "cargo test --all",
    options: approvalOptions,
    metadata: {
      command: "cargo test --all",
      cwd: "/repo",
      reason: "Run the verification suite",
      sandbox_label: "workspace sandbox",
    },
    ...overrides,
  });
}

export function sandboxApprovalFixture(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return toolApprovalFixture({
    id: "sandbox-approval-fixture",
    kind: "sandbox_approval",
    title: "Sandbox approval required",
    metadata: {
      command: "curl https://example.test",
      cwd: "/repo",
      reason: "The command requires network access.",
      sandbox_label: "outside sandbox",
    },
    ...overrides,
  });
}

export function userInputFixture(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return interactionFixture({
    id: "user-input-fixture",
    kind: "request_user_input",
    title: "User input required",
    options: [],
    metadata: {
      questions: [
        {
          id: "risk_profile",
          prompt: "Which risk profile should be used?",
          kind: "single",
          isOther: true,
          isSecret: false,
          options: [
            { id: "low", label: "Low" },
            { id: "high", label: "High" },
          ],
        },
        {
          id: "additional_context",
          prompt: "What additional context should the agent consider?",
          kind: "text",
          isOther: false,
          isSecret: false,
          options: [],
        },
        {
          id: "deploy_token",
          header: "Secret",
          prompt: "Paste the deploy token",
          kind: "text",
          isOther: false,
          isSecret: true,
          options: [],
        },
      ],
    },
    ...overrides,
  });
}

export function interactionSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({ interactions: [toolApprovalFixture()], ...overrides });
}

export function approvalResponse(
  selectedOption: "approve" | "deny" | "abort",
  applyToFuture: CodeUiApplyToFuture = "no",
): CodeUiInteractionResponse {
  return {
    approved: selectedOption === "approve" ? true : selectedOption === "deny" ? false : undefined,
    selectedOption,
    applyToFuture,
    answers: {},
  };
}

export const approveResponse = (applyToFuture?: CodeUiApplyToFuture) =>
  approvalResponse("approve", applyToFuture);
export const denyResponse = (applyToFuture?: CodeUiApplyToFuture) =>
  approvalResponse("deny", applyToFuture);
export const abortResponse = (applyToFuture?: CodeUiApplyToFuture) =>
  approvalResponse("abort", applyToFuture);
