"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import {
  allowSseSnapshotFetch,
  initialSseResilienceState,
  reduceSseResilience,
  subscribeSseDisconnect,
  subscribeSseResync,
  subscribeSseResyncComplete,
  type SseResilienceState,
} from "@/lib/code-ui/sse-resilience";
import { useCodeUiStore } from "@/lib/code-ui/store";

import { SseResilienceHost } from "./SseResilienceHost";

function errorMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Resync failed. Try again.";
}

export interface SessionSseResilienceProps {
  /** Test seam — inject a starting resilience state (fixture-driven). */
  initialState?: SseResilienceState;
}

/**
 * Observes the shared store without replacing W2-07 reconnect internals.
 * Disconnects and W3-08 resync events are observed via
 * `wrapClientForSseResilience` (page injects the wrapped client). Sequence
 * numbers come only from the v2 wire cursor and never decrease across
 * reconnect replay (synthetic seq 0).
 */
export function SessionSseResilience({ initialState }: SessionSseResilienceProps) {
  const { snapshot, refresh, subscribe } = useCodeUiStore();
  const [state, setState] = useState<SseResilienceState>(
    () => initialState ?? initialSseResilienceState({ snapshotRetained: Boolean(snapshot) }),
  );
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string>();
  const hadSnapshot = useRef(Boolean(snapshot));

  useEffect(() => {
    if (!snapshot) return;
    hadSnapshot.current = true;
    setState((current) =>
      current.snapshotRetained ? current : { ...current, snapshotRetained: true },
    );
  }, [snapshot]);

  useEffect(() => {
    return subscribe("*", (event) => {
      setState((current) =>
        reduceSseResilience(current, {
          type: "stream_event",
          seq: event.seq,
        }),
      );
    });
  }, [subscribe]);

  useEffect(() => {
    return subscribeSseDisconnect(() => {
      setState((current) =>
        reduceSseResilience(current, {
          type: "stream_disconnect",
          snapshotRetained: hadSnapshot.current || Boolean(snapshot),
        }),
      );
    });
  }, [snapshot]);

  useEffect(() => {
    return subscribeSseResync((event) => {
      setState((current) =>
        reduceSseResilience(current, {
          type: "backlog_overflow",
          hint: event.reason || "SSE backlog exceeded; request an explicit snapshot resync.",
          lastSeq: event.lastCursor,
        }),
      );
    });
  }, []);

  useEffect(() => {
    return subscribeSseResyncComplete((seq) => {
      setState((current) =>
        reduceSseResilience(current, {
          type: "resync_completed",
          seq,
        }),
      );
    });
  }, []);

  const onResync = useCallback(async () => {
    setBusy(true);
    setActionError(undefined);
    setState((current) => reduceSseResilience(current, { type: "resync_requested" }));
    allowSseSnapshotFetch();
    try {
      let result = await refresh();
      // A concurrent reconnect refresh may own the first attempt — retry once.
      if (result === "raced") {
        result = await refresh();
      }
      if (result === "failed" || result === "raced") {
        throw new Error("Unable to refresh Code UI session snapshot");
      }
      setState((current) =>
        reduceSseResilience(current, {
          type: "resync_completed",
          seq: current.cursor.lastSeq,
        }),
      );
    } catch (cause) {
      setActionError(errorMessage(cause));
      setState((current) =>
        reduceSseResilience(current, {
          type: "resync_failed",
          snapshotRetained:
            current.snapshotRetained || hadSnapshot.current || Boolean(snapshot),
          message: errorMessage(cause),
        }),
      );
    } finally {
      setBusy(false);
    }
  }, [refresh, snapshot]);

  return (
    <SseResilienceHost state={state} busy={busy} error={actionError} onResync={onResync} />
  );
}
