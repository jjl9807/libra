"use client";

import type { SseResilienceState } from "../../../lib/code-ui/sse-resilience";

import { SseResilienceBanner } from "./SseResilienceBanner";

export interface SseResilienceHostProps {
  state: SseResilienceState;
  busy?: boolean;
  error?: string;
  onResync(): void | Promise<void>;
}

export function SseResilienceHost({
  state,
  busy,
  error,
  onResync,
}: SseResilienceHostProps) {
  return (
    <div aria-label="SSE resilience workspace">
      <SseResilienceBanner state={state} busy={busy} error={error} onResync={onResync} />
    </div>
  );
}

export { SseResilienceBanner } from "./SseResilienceBanner";
