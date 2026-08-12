"use client";

import type { SseResilienceState } from "../../../lib/code-ui/sse-resilience";

export interface SseResilienceBannerProps {
  state: SseResilienceState;
  busy?: boolean;
  error?: string;
  onResync(): void | Promise<void>;
}

function statusLabel(status: SseResilienceState["status"]): string {
  switch (status) {
    case "connected":
      return "SSE connected";
    case "reconnecting":
      return "SSE reconnecting";
    case "resync_required":
      return "SSE resync required";
    case "resynced":
      return "SSE resynced";
  }
}

export function SseResilienceBanner({
  state,
  busy = false,
  error,
  onResync,
}: SseResilienceBannerProps) {
  return (
    <section aria-label="SSE resilience panel" aria-live="polite">
      <h2>SSE resilience</h2>
      <p>{statusLabel(state.status)}</p>
      {typeof state.cursor.lastSeq === "number" ? (
        <p>Last cursor seq: {state.cursor.lastSeq}</p>
      ) : (
        <p>No cursor seq observed yet.</p>
      )}
      <p>
        {state.snapshotRetained
          ? "Projected session snapshot is retained during the outage."
          : "No projected session snapshot is retained yet."}
      </p>
      {state.backlogHint ? <p role="status">{state.backlogHint}</p> : null}
      {(state.status === "resync_required" || state.status === "reconnecting") && (
        <button type="button" disabled={busy} onClick={() => void onResync()}>
          Resync snapshot
        </button>
      )}
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
