import { sseFixture } from "../fixtures";
import type { CodeUiEventEnvelope } from "../types";

import {
  initialSseResilienceState,
  reduceSseResilience,
  type SseResilienceEvent,
  type SseResilienceState,
} from "./state";

export function connectedSseState(): SseResilienceState {
  return initialSseResilienceState({
    status: "connected",
    cursor: { lastSeq: 1 },
    snapshotRetained: true,
  });
}

export function reconnectingSseState(): SseResilienceState {
  return initialSseResilienceState({
    status: "reconnecting",
    cursor: { lastSeq: 4 },
    snapshotRetained: true,
  });
}

export function resyncRequiredSseState(): SseResilienceState {
  return initialSseResilienceState({
    status: "resync_required",
    cursor: { lastSeq: 12 },
    snapshotRetained: true,
    backlogHint: "SSE backlog exceeded; request an explicit snapshot resync.",
  });
}

export function resyncedSseState(): SseResilienceState {
  return initialSseResilienceState({
    status: "resynced",
    cursor: { lastSeq: 20 },
    snapshotRetained: true,
  });
}

/** Cursor reconnect scenario: disconnect keeps cursor, next event restores connected. */
export function cursorReconnectScenario(): {
  start: SseResilienceState;
  events: SseResilienceEvent[];
  envelopes: CodeUiEventEnvelope[];
} {
  const start = reduceSseResilience(initialSseResilienceState(), {
    type: "stream_event",
    seq: 7,
  });
  return {
    start,
    events: [
      { type: "stream_disconnect", snapshotRetained: true },
      { type: "stream_event", seq: 8 },
    ],
    envelopes: [
      sseFixture(undefined, { seq: 7 }),
      sseFixture(undefined, { seq: 8 }),
    ],
  };
}

/** Backlog overflow then explicit snapshot resync. */
export function backlogResyncScenario(): {
  start: SseResilienceState;
  events: SseResilienceEvent[];
} {
  return {
    start: connectedSseState(),
    events: [
      {
        type: "backlog_overflow",
        hint: "SSE backlog exceeded; request an explicit snapshot resync.",
        lastSeq: 12,
      },
      { type: "resync_requested" },
      { type: "resync_completed", seq: 12 },
    ],
  };
}

export function foldSseEvents(
  start: SseResilienceState,
  events: SseResilienceEvent[],
): SseResilienceState {
  return events.reduce(reduceSseResilience, start);
}
