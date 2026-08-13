export type IsoTimestamp = string;
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

export type CodeUiSessionStatus =
  | "idle"
  | "thinking"
  | "executing_tool"
  | "awaiting_interaction"
  | "completed"
  | "error"
  | "indeterminate_side_effect";
export type CodeUiControllerKind = "none" | "browser" | "automation" | "tui" | "cli";
export type CodeUiTranscriptEntryKind =
  | "user_message"
  | "assistant_message"
  | "tool_call"
  | "plan_summary"
  | "diff"
  | "info_note";
export type CodeUiInteractionKind =
  | "approval"
  | "sandbox_approval"
  | "request_user_input"
  | "intent_review_choice"
  | "post_plan_choice"
  | "plan_execution_repair";
export type CodeUiInteractionStatus = "pending" | "resolved" | "cancelled";
export type CodeUiApplyToFuture = "no" | "accept_all" | "decline_all";
export type CodeUiEventType =
  | "session_updated"
  | "status_changed"
  | "controller_changed";

export interface CodeUiCapabilities {
  messageInput: boolean;
  streamingText: boolean;
  planUpdates: boolean;
  toolCalls: boolean;
  patchsets: boolean;
  interactiveApprovals: boolean;
  structuredQuestions: boolean;
  providerSessionResume: boolean;
  commandIdempotency: boolean;
}

export interface CodeUiProviderInfo {
  provider: string;
  model?: string;
  mode?: string;
  managed: boolean;
}

export interface CodeUiControllerState {
  kind: CodeUiControllerKind;
  ownerLabel?: string;
  canWrite: boolean;
  leaseExpiresAt?: IsoTimestamp;
  reason?: string;
  loopbackOnly: boolean;
}

export interface CodeUiTranscriptEntry {
  id: string;
  kind: CodeUiTranscriptEntryKind;
  title?: string;
  content?: string;
  status?: string;
  streaming: boolean;
  metadata: JsonValue;
  createdAt: IsoTimestamp;
  updatedAt: IsoTimestamp;
}

export interface CodeUiInteractionOption {
  id: string;
  label: string;
  description?: string;
}

export interface CodeUiInteractionRequest {
  id: string;
  kind: CodeUiInteractionKind;
  title?: string;
  description?: string;
  prompt?: string;
  options: CodeUiInteractionOption[];
  status: CodeUiInteractionStatus;
  metadata: JsonValue;
  requestedAt: IsoTimestamp;
  resolvedAt?: IsoTimestamp;
}

export interface CodeUiInteractionResponse {
  approved?: boolean;
  applyToFuture?: CodeUiApplyToFuture;
  selectedOption?: string;
  maxAttempts?: number;
  note?: string;
  answers: Record<string, string[]>;
}

export interface CodeUiPlanSnapshot {
  id: string;
  title?: string;
  summary?: string;
  status: string;
  steps: Array<{ step: string; status: string }>;
  updatedAt: IsoTimestamp;
}
export interface CodeUiTaskSnapshot {
  id: string;
  title?: string;
  status: string;
  details?: string;
  updatedAt: IsoTimestamp;
}
export interface CodeUiToolCallSnapshot {
  id: string;
  toolName: string;
  status: string;
  summary?: string;
  details?: string;
  updatedAt: IsoTimestamp;
}
export interface CodeUiPatchsetSnapshot {
  id: string;
  status: string;
  changes: Array<{ path: string; changeType: string; diff?: string }>;
  updatedAt: IsoTimestamp;
}
export interface CodeUiThreadGraphNode {
  depth: number;
  kind: string;
  id: string;
  label: string;
  tags?: string[];
}
export interface CodeUiThreadGraph {
  threadId: string;
  title?: string;
  selectedPlanId?: string;
  activeTaskId?: string;
  activeRunId?: string;
  nodes: CodeUiThreadGraphNode[];
  truncated?: boolean;
  omittedNodeCount?: number;
  totalNodeCount?: number;
}

export interface CodeUiPlanExecutionRepair {
  state:
    | "automatic_repair"
    | "awaiting_user"
    | "intent_spec_revision"
    | "manual_action"
    | "cancelled";
  interaction_id?: string;
  route?: "plan_revision" | "intent_spec_revision" | "manual_action";
  evidence: {
    output: string;
    diagnostics: string[];
    attempt: number;
    max_attempts: number;
  };
}

export interface CodeUiSessionSnapshot {
  sessionId: string;
  threadId?: string;
  workingDir: string;
  provider: CodeUiProviderInfo;
  capabilities: CodeUiCapabilities;
  controller: CodeUiControllerState;
  status: CodeUiSessionStatus;
  transcript: CodeUiTranscriptEntry[];
  plans: CodeUiPlanSnapshot[];
  tasks: CodeUiTaskSnapshot[];
  toolCalls: CodeUiToolCallSnapshot[];
  patchsets: CodeUiPatchsetSnapshot[];
  interactions: CodeUiInteractionRequest[];
  planExecutionRepair?: CodeUiPlanExecutionRepair;
  threadGraph?: CodeUiThreadGraph;
  updatedAt: IsoTimestamp;
}

export interface CodeUiEventEnvelope<T = CodeUiSessionSnapshot> {
  seq: number;
  type: CodeUiEventType | (string & {});
  at: IsoTimestamp;
  data: T;
}

export interface CodeUiControllerAttachResponse {
  controllerToken: string;
  leaseExpiresAt: IsoTimestamp;
  controller: CodeUiControllerState;
}

export interface CodeUiAckResponse {
  accepted: boolean;
}

export interface CodeUiApiError {
  code?: string;
  message: string;
  status: number;
}
