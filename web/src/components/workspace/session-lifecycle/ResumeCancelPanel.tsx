"use client";

import type { ResumeAffordance } from "../../../lib/code-ui/session-lifecycle";

export interface ResumeCancelPanelProps {
  currentThreadId?: string;
  selectedThreadId?: string;
  phaseLabel: string;
  affordance: ResumeAffordance;
  selectionError?: string;
  cancelError?: string;
  resumeHint?: string;
  busy?: boolean;
  canCancel?: boolean;
  onCancel(): void | Promise<void>;
  onResumeIntent(): void | Promise<void>;
}

export function ResumeCancelPanel({
  currentThreadId,
  selectedThreadId,
  phaseLabel,
  affordance,
  selectionError,
  cancelError,
  resumeHint,
  busy = false,
  canCancel = true,
  onCancel,
  onResumeIntent,
}: ResumeCancelPanelProps) {
  const resumeDisabled = busy || affordance.kind !== "ready" || Boolean(selectionError);

  return (
    <section aria-label="Session resume cancel panel">
      <h2>Session lifecycle</h2>
      <p>
        Active thread: {currentThreadId ?? "none"} · Phase: {phaseLabel}
      </p>
      <p>
        Selected thread: {selectedThreadId ?? "none"}
      </p>
      <p aria-live="polite">{affordance.reason}</p>
      {resumeHint ? <p>{resumeHint}</p> : null}
      <button
        type="button"
        disabled={resumeDisabled}
        onClick={() => void onResumeIntent()}
      >
        Prepare resume
      </button>
      <button
        type="button"
        disabled={busy || !canCancel}
        onClick={() => void onCancel()}
      >
        Cancel turn
      </button>
      {selectionError ? <p role="alert">{selectionError}</p> : null}
      {cancelError ? <p role="alert">{cancelError}</p> : null}
    </section>
  );
}
