/** Fixture-driven SSE resilience UI states. Production v2 wire is W3-06/W3-08. */

export type SseResilienceStatus =
  | "connected"
  | "reconnecting"
  | "resync_required"
  | "resynced";

export interface SseCursor {
  /** Last observed envelope seq from the wire — never invented locally. */
  lastSeq?: number;
}

export interface SseResilienceState {
  status: SseResilienceStatus;
  cursor: SseCursor;
  /** Human-readable backlog / lag hint when the stream cannot catch up. */
  backlogHint?: string;
  /** True when a projected session snapshot is still available during outages. */
  snapshotRetained: boolean;
  /**
   * After a disconnect, accept the first nonzero wire seq even if it is lower
   * than the retained cursor (server restart). Synthetic replay seq 0 is ignored.
   */
  acceptCursorReset?: boolean;
}

export type SseResilienceEvent =
  | { type: "stream_open" }
  | { type: "stream_event"; seq: number }
  | { type: "stream_disconnect"; snapshotRetained: boolean }
  | { type: "backlog_overflow"; hint: string; lastSeq?: number }
  | { type: "resync_requested" }
  | { type: "resync_completed"; seq?: number }
  | { type: "resync_failed"; snapshotRetained: boolean; message?: string };

export function initialSseResilienceState(
  overrides: Partial<SseResilienceState> = {},
): SseResilienceState {
  return {
    status: "connected",
    cursor: {},
    snapshotRetained: false,
    acceptCursorReset: false,
    ...overrides,
  };
}

/**
 * Pure reducer for fixture scenarios. Does not invent sequence numbers —
 * only records `seq` values supplied by the wire/fixture event.
 */
export function reduceSseResilience(
  state: SseResilienceState,
  event: SseResilienceEvent,
): SseResilienceState {
  switch (event.type) {
    case "stream_open":
      return {
        ...state,
        status: state.status === "resync_required" ? state.status : "connected",
        backlogHint: state.status === "resync_required" ? state.backlogHint : undefined,
      };
    case "stream_event": {
      const previous = state.cursor.lastSeq;
      let lastSeq: number;
      let acceptCursorReset = state.acceptCursorReset === true;
      if (event.seq === 0 && previous !== undefined) {
        // Synthetic reconnect replay — keep the retained cursor.
        lastSeq = previous;
      } else if (acceptCursorReset && event.seq > 0) {
        // First nonzero seq after disconnect may restart after a server reboot.
        lastSeq = event.seq;
        acceptCursorReset = false;
      } else if (previous === undefined) {
        lastSeq = event.seq;
      } else {
        lastSeq = Math.max(previous, event.seq);
      }
      return {
        ...state,
        status: state.status === "resync_required" ? "resync_required" : "connected",
        cursor: { lastSeq },
        backlogHint: state.status === "resync_required" ? state.backlogHint : undefined,
        snapshotRetained: true,
        acceptCursorReset,
      };
    }
    case "stream_disconnect":
      return {
        ...state,
        status: "reconnecting",
        snapshotRetained: event.snapshotRetained,
        acceptCursorReset: true,
        // Keep cursor so a later reconnect can display the last known seq until
        // the first nonzero post-reconnect event arrives.
      };
    case "backlog_overflow":
      return {
        ...state,
        status: "resync_required",
        backlogHint: event.hint,
        cursor: event.lastSeq === undefined ? state.cursor : { lastSeq: event.lastSeq },
        snapshotRetained: true,
      };
    case "resync_requested":
      return {
        ...state,
        status: "reconnecting",
      };
    case "resync_completed":
      return {
        // Snapshot pull succeeded; if the stream is still down (post-disconnect),
        // stay reconnecting until a wire event arrives.
        status: state.acceptCursorReset ? "reconnecting" : "resynced",
        cursor: event.seq === undefined ? state.cursor : { lastSeq: event.seq },
        backlogHint: undefined,
        snapshotRetained: true,
        acceptCursorReset: state.acceptCursorReset,
      };
    case "resync_failed":
      return {
        ...state,
        status: event.snapshotRetained ? "resync_required" : "reconnecting",
        snapshotRetained: event.snapshotRetained,
        backlogHint: event.message ?? state.backlogHint,
      };
  }
}
