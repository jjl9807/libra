import { phaseForSnapshot, type CodeUiPhase } from "../phases";
import type { CodeUiSessionSnapshot } from "../types";

export type ResumeAffordance =
  | { kind: "unsupported"; reason: string }
  | { kind: "deferred"; reason: string }
  | { kind: "ready"; reason: string };

/**
 * Browser resume HTTP is W3-01. Until then the UI only classifies whether a
 * thread is a valid resume target and explains the process-level `--resume`
 * path when the capability is advertised.
 *
 * `/api/code/threads` is repository-storage-scoped (shared across linked
 * worktrees). CLI `--resume` is working-directory scoped, so callers must
 * launch from the original session cwd — this UI cannot filter foreign
 * worktree threads until the wire exposes per-thread workingDir (W3-01).
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
        "Browser resume HTTP lands in W3-01. Until then restart with `libra code --resume <thread_id>` from that session's original working directory (thread list is repo-shared across worktrees).",
    };
  }
  return {
    kind: "deferred",
    reason: `Resume is not available while the session phase is ${phase}.`,
  };
}

export function resumeLaunchHint(
  threadId: string,
  workingDir?: string,
): string {
  const cwdNote = workingDir?.trim()
    ? ` This thread matches the live session cwd \`${workingDir.trim()}\`.`
    : " Launch from that thread's original working directory (per-thread cwd is not on the wire yet; do not assume the live session cwd).";
  return `Resume target ${threadId} is ready.${cwdNote} Restart with \`libra code --resume ${threadId}\` (browser resume HTTP: W3-01). Thread list is repository-shared across worktrees.`;
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
