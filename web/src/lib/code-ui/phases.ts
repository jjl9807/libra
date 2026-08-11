import type { CodeUiSessionSnapshot, CodeUiSessionStatus } from "./types";

export type CodeUiPhase =
  | "ready"
  | "thinking"
  | "executing"
  | "waiting"
  | "finished"
  | "failed"
  | "reconcile";

const PHASES: Record<CodeUiSessionStatus, CodeUiPhase> = {
  idle: "ready",
  thinking: "thinking",
  executing_tool: "executing",
  awaiting_interaction: "waiting",
  completed: "finished",
  error: "failed",
  indeterminate_side_effect: "reconcile",
};

export function phaseForStatus(status: CodeUiSessionStatus): CodeUiPhase {
  return PHASES[status];
}

export function phaseForSnapshot(snapshot?: CodeUiSessionSnapshot): CodeUiPhase {
  return snapshot ? phaseForStatus(snapshot.status) : "ready";
}

export function isTerminalPhase(phase: CodeUiPhase): boolean {
  return phase === "finished" || phase === "failed" || phase === "reconcile";
}
