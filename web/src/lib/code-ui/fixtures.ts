import type {
  CodeUiEventEnvelope,
  CodeUiInteractionRequest,
  CodeUiPlanExecutionRepair,
  CodeUiSessionSnapshot,
  CodeUiToolCallSnapshot,
  CodeUiTranscriptEntry,
} from "./types";

const now = "2026-07-15T00:00:00.000Z";

export function sessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return {
    sessionId: "session-fixture",
    workingDir: "/repo",
    provider: { provider: "test", managed: false },
    capabilities: {
      messageInput: true, streamingText: true, planUpdates: true, toolCalls: true,
      patchsets: true, interactiveApprovals: true, structuredQuestions: true,
      providerSessionResume: false, commandIdempotency: true,
    },
    controller: { kind: "none", canWrite: false, loopbackOnly: true },
    status: "idle",
    transcript: [],
    plans: [],
    tasks: [],
    toolCalls: [],
    patchsets: [],
    interactions: [],
    updatedAt: now,
    ...overrides,
  };
}

export function interactionFixture(
  overrides: Partial<CodeUiInteractionRequest> = {},
): CodeUiInteractionRequest {
  return {
    id: "interaction-fixture", kind: "request_user_input", options: [], status: "pending",
    metadata: {}, requestedAt: now, ...overrides,
  };
}

export function commandFixture(overrides: Partial<CodeUiToolCallSnapshot> = {}): CodeUiToolCallSnapshot {
  return { id: "command-fixture", toolName: "shell", status: "running", updatedAt: now, ...overrides };
}

export function queryFixture(overrides: Partial<CodeUiTranscriptEntry> = {}): CodeUiTranscriptEntry {
  return {
    id: "query-fixture", kind: "assistant_message", content: "query", streaming: false,
    metadata: {}, createdAt: now, updatedAt: now, ...overrides,
  };
}

export function lifecycleFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({ threadId: "thread-fixture", ...overrides });
}

export function executionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({ status: "executing_tool", toolCalls: [commandFixture()], ...overrides });
}

export function repairFixture(
  overrides: Partial<CodeUiPlanExecutionRepair> = {},
): CodeUiPlanExecutionRepair {
  return {
    state: "awaiting_user",
    interaction_id: "repair-interaction",
    route: "plan_revision",
    evidence: { output: "verification failed", diagnostics: ["test failure"], attempt: 1, max_attempts: 2 },
    ...overrides,
  };
}

export function sseFixture(
  snapshot: CodeUiSessionSnapshot = sessionFixture(),
  overrides: Partial<CodeUiEventEnvelope> = {},
): CodeUiEventEnvelope {
  return { seq: 1, type: "session_updated", at: now, data: snapshot, ...overrides };
}
