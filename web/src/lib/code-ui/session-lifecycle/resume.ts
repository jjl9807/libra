import { phaseForSnapshot, type CodeUiPhase } from "../phases";
import type { CodeUiSessionSnapshot } from "../types";

export type ResumeAffordance =
  | { kind: "unsupported"; reason: string }
  | { kind: "deferred"; reason: string }
  | { kind: "ready"; reason: string };

/**
 * Classifies whether a thread is a valid browser resume target before
 * `POST /api/code/session/resume`. Callers still need the active controller
 * lease. In-process runtime swap may fail closed with
 * `SESSION_RESUME_REQUIRES_RESTART` (CLI restart hint).
 *
 * `/api/code/threads` is repository-storage-scoped (shared across linked
 * worktrees). CLI `--resume` is working-directory scoped. Per-thread
 * `workingDir` is omitted on the wire until projections persist it.
 */
export function resumeAffordance(snapshot?: CodeUiSessionSnapshot): ResumeAffordance {
  if (!snapshot) {
    return { kind: "unsupported", reason: "No live session snapshot is loaded." };
  }
  if (!snapshot.capabilities.providerSessionResume) {
    return {
      kind: "unsupported",
      reason: "This session does not advertise providerSessionResume.",
    };
  }
  const phase = phaseForSnapshot(snapshot);
  if (phase === "thinking" || phase === "executing") {
    return {
      kind: "deferred",
      reason: "Wait for the active turn to settle before resuming another thread.",
    };
  }
  if (phase === "reconcile") {
    return {
      kind: "unsupported",
      reason:
        "This session needs inspection and reconciliation before resume (indeterminate side effect).",
    };
  }
  if (phase === "finished" || phase === "failed" || phase === "waiting" || phase === "ready") {
    return {
      kind: "ready",
      reason:
        "Select this thread through POST /api/code/session/resume with a controller lease. Resume remains working-directory scoped (thread lists are repo-shared across worktrees).",
    };
  }
  return {
    kind: "deferred",
    reason: `Resume is not available while the session phase is ${phase}.`,
  };
}

export function resumeLaunchHint(threadId: string, workingDir?: string): string {
  const cwdNote = workingDir?.trim()
    ? ` This thread matches the live session cwd \`${workingDir.trim()}\`.`
    : " Launch from that thread's original working directory (per-thread cwd is not on the wire yet; do not assume the live session cwd).";
  return `Resume target ${threadId} is ready.${cwdNote} Use POST /api/code/session/resume with { threadId } and the active controller lease. Thread list is repository-shared across worktrees.`;
}

export function canSelectThreadForResume(
  affordance: ResumeAffordance,
  threadId: string,
  currentThreadId?: string,
): string | undefined {
  if (!threadId.trim()) return "Select a thread before resuming.";
  if (currentThreadId && threadId === currentThreadId) {
    return "That thread is already the active session.";
  }
  if (affordance.kind === "unsupported") return affordance.reason;
  if (affordance.kind === "deferred") return affordance.reason;
  return undefined;
}

export function phaseLabel(phase: CodeUiPhase): string {
  switch (phase) {
    case "ready":
      return "Ready";
    case "thinking":
      return "Thinking";
    case "executing":
      return "Executing";
    case "waiting":
      return "Awaiting interaction";
    case "finished":
      return "Finished";
    case "failed":
      return "Failed";
    case "reconcile":
      return "Needs reconciliation";
  }
}
