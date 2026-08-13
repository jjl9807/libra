"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import {
  INDEXED_THREAD_GRAPH_LOAD_FAILED,
  threadGraphCoversSnapshotHeads,
  threadGraphView,
} from "@/lib/code-ui/thread-graph";
import { useCodeUiStore } from "@/lib/code-ui/store";
import type { CodeUiSessionSnapshot, CodeUiThreadGraph } from "@/lib/code-ui/types";

import { ThreadGraphHost } from "./ThreadGraphHost";

type FetchedThreadGraph = {
  threadId: string;
  graph?: CodeUiThreadGraph;
  error?: string;
};

const NOT_FOUND_BACKOFF_MS = [1_000, 2_000, 4_000, 8_000];
const UNAVAILABLE_BACKOFF_MS = [1_000, 2_000];
const REVALIDATE_DEBOUNCE_MS = 2_000;

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

export function SessionThreadGraph() {
  const { snapshot } = useCodeUiStore();
  const [fetched, setFetched] = useState<FetchedThreadGraph | undefined>();
  const threadId = snapshot?.threadId;
  const hasSnapshotGraph = Boolean(snapshot?.threadGraph);
  const lineageKey = [
    threadId ?? "",
    snapshot?.plans.map((plan) => `${plan.id}:${plan.status}`).join("\0") ?? "",
    snapshot?.tasks.map((task) => `${task.id}:${task.status}`).join("\0") ?? "",
    snapshot?.patchsets.map((patchset) => `${patchset.id}:${patchset.status}`).join("\0") ?? "",
  ].join("|");
  const prevLineageRef = useRef<string | undefined>(undefined);
  const revalidateTimerRef = useRef<number | undefined>(undefined);
  const graphAbortRef = useRef<AbortController | undefined>(undefined);

  useEffect(() => {
    if (!threadId || hasSnapshotGraph) {
      return;
    }
    let cancelled = false;
    let retryTimer: number | undefined;
    let notFoundAttempts = 0;
    let unavailableAttempts = 0;

    const schedule = (delayMs: number, next: () => void) => {
      retryTimer = window.setTimeout(next, delayMs);
    };

    const run = () => {
      graphAbortRef.current?.abort();
      const controller = new AbortController();
      graphAbortRef.current = controller;
      void fetch(`/api/code/thread-graph?threadId=${encodeURIComponent(threadId)}`, {
        credentials: "same-origin",
        signal: controller.signal,
      })
        .then(async (response) => {
          if (response.ok) {
            return {
              ok: true as const,
              graph: (await response.json()) as CodeUiThreadGraph,
            };
          }
          let code: string | undefined;
          try {
            const body = (await response.json()) as { error?: { code?: string; message?: string } };
            if (typeof body.error?.code === "string") code = body.error.code;
          } catch {
            // Non-JSON failures still surface as availability errors.
          }
          return { ok: false as const, status: response.status, code };
        })
        .then((result) => {
          if (cancelled || controller.signal.aborted) return;
          if (result.ok) {
            setFetched({ threadId, graph: result.graph });
            return;
          }
          if (result.status === 404 || result.code === "THREAD_GRAPH_NOT_FOUND") {
            setFetched({ threadId });
            if (notFoundAttempts < NOT_FOUND_BACKOFF_MS.length) {
              const delay = NOT_FOUND_BACKOFF_MS[notFoundAttempts];
              notFoundAttempts += 1;
              schedule(delay, run);
            }
            return;
          }
          const suffix = result.code ? ` (${result.code})` : "";
          setFetched({
            threadId,
            error: `${INDEXED_THREAD_GRAPH_LOAD_FAILED}${suffix}`,
          });
          if (unavailableAttempts < UNAVAILABLE_BACKOFF_MS.length) {
            const delay = UNAVAILABLE_BACKOFF_MS[unavailableAttempts];
            unavailableAttempts += 1;
            schedule(delay, run);
          }
        })
        .catch((error: unknown) => {
          if (cancelled || isAbortError(error)) return;
          setFetched({
            threadId,
            error: INDEXED_THREAD_GRAPH_LOAD_FAILED,
          });
          if (unavailableAttempts < UNAVAILABLE_BACKOFF_MS.length) {
            const delay = UNAVAILABLE_BACKOFF_MS[unavailableAttempts];
            unavailableAttempts += 1;
            schedule(delay, run);
          }
        });
    };

    run();
    return () => {
      cancelled = true;
      graphAbortRef.current?.abort();
      graphAbortRef.current = undefined;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
    };
  }, [threadId, hasSnapshotGraph, lineageKey]);

  useEffect(() => {
    if (!threadId || !hasSnapshotGraph) {
      prevLineageRef.current = lineageKey;
      return;
    }
    const previous = prevLineageRef.current;
    prevLineageRef.current = lineageKey;
    if (previous === undefined || previous === lineageKey) {
      return;
    }
    if (revalidateTimerRef.current !== undefined) {
      window.clearTimeout(revalidateTimerRef.current);
    }
    revalidateTimerRef.current = window.setTimeout(() => {
      revalidateTimerRef.current = undefined;
      graphAbortRef.current?.abort();
      const controller = new AbortController();
      graphAbortRef.current = controller;
      void fetch(`/api/code/thread-graph?threadId=${encodeURIComponent(threadId)}`, {
        credentials: "same-origin",
        signal: controller.signal,
      })
        .then((response) => (response.ok ? response.json() : Promise.reject()))
        .then((graph: CodeUiThreadGraph) => {
          if (controller.signal.aborted) return;
          setFetched({ threadId, graph });
        })
        .catch((error: unknown) => {
          if (isAbortError(error)) return;
        })
        .finally(() => {
          if (graphAbortRef.current === controller) {
            graphAbortRef.current = undefined;
          }
        });
    }, REVALIDATE_DEBOUNCE_MS);
  }, [threadId, hasSnapshotGraph, lineageKey]);

  useEffect(() => {
    return () => {
      prevLineageRef.current = undefined;
      graphAbortRef.current?.abort();
      graphAbortRef.current = undefined;
      if (revalidateTimerRef.current !== undefined) {
        window.clearTimeout(revalidateTimerRef.current);
        revalidateTimerRef.current = undefined;
      }
    };
  }, [threadId]);

  const viewSnapshot = useMemo((): CodeUiSessionSnapshot | undefined => {
    if (!snapshot || !fetched || fetched.threadId !== snapshot.threadId || !fetched.graph) {
      return snapshot;
    }
    const overlaid = { ...snapshot, threadGraph: fetched.graph };
    return threadGraphCoversSnapshotHeads(overlaid) ? overlaid : snapshot;
  }, [snapshot, fetched]);

  const view = useMemo(
    () =>
      threadGraphView(viewSnapshot, {
        loadError:
          snapshot?.threadGraph || fetched?.threadId !== snapshot?.threadId
            ? undefined
            : fetched?.error,
      }),
    [viewSnapshot, snapshot?.threadGraph, snapshot?.threadId, fetched],
  );
  return <ThreadGraphHost view={view} />;
}
