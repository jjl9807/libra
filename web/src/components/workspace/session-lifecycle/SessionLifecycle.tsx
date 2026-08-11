"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useBrowserController } from "@/lib/code-ui/controller";
import { phaseForSnapshot } from "@/lib/code-ui/phases";
import {
  canSelectThreadForResume,
  createSessionLifecycleApi,
  phaseLabel,
  resumeAffordance,
  type SessionLifecycleApi,
  type ThreadListItem,
} from "@/lib/code-ui/session-lifecycle";
import { useCodeUiStore } from "@/lib/code-ui/store";

import { SessionLifecycleHost } from "./SessionLifecycleHost";

const PAGE_SIZE = 50;

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Request failed. Try again.";
}

export interface SessionLifecycleProps {
  /** Test seam — production leaves this unset and uses same-origin fetch. */
  api?: SessionLifecycleApi;
}

/**
 * Session thread list + cancel/resume affordances for W2-10 / W3-01.
 * Resume posts `POST /api/code/session/resume` under the controller lease;
 * cancel uses the shared controller.
 */
export function SessionLifecycle({ api: injectedApi }: SessionLifecycleProps) {
  const { snapshot } = useCodeUiStore();
  const controller = useBrowserController();
  const api = useMemo(() => injectedApi ?? createSessionLifecycleApi(), [injectedApi]);

  const [items, setItems] = useState<ThreadListItem[]>([]);
  const [nextOffset, setNextOffset] = useState<number | undefined>();
  const [selectedThreadId, setSelectedThreadId] = useState<string>();
  const [listError, setListError] = useState<string>();
  const [cancelError, setCancelError] = useState<string>();
  const [selectionError, setSelectionError] = useState<string>();
  const [resumeHint, setResumeHint] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  const busyRef = useRef(false);
  const listGeneration = useRef(0);

  const affordance = useMemo(() => resumeAffordance(snapshot), [snapshot]);
  const currentThreadId = snapshot?.threadId;
  const phase = phaseForSnapshot(snapshot);
  const canCancel = Boolean(snapshot?.controller.canWrite);

  const run = useCallback(async (operation: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await operation();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  const loadThreads = useCallback(
    async (options?: { append?: boolean; offset?: number }) => {
      const generation = ++listGeneration.current;
      setLoading(true);
      setListError(undefined);
      try {
        const response = await api.listThreads(PAGE_SIZE, options?.offset ?? 0);
        if (generation !== listGeneration.current) return;
        setItems((prev) => {
          const merged = options?.append ? [...prev, ...response.items] : response.items;
          const seen = new Set<string>();
          return merged.filter((item) => {
            if (seen.has(item.id)) return false;
            seen.add(item.id);
            return true;
          });
        });
        setNextOffset(response.nextOffset);
      } catch (cause) {
        if (generation !== listGeneration.current) return;
        setListError(errorMessage(cause));
        if (!options?.append) setItems([]);
        setNextOffset(undefined);
      } finally {
        if (generation === listGeneration.current) setLoading(false);
      }
    },
    [api],
  );

  useEffect(() => {
    void loadThreads();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional mount-only thread probe
  }, []);

  useEffect(() => {
    if (!selectedThreadId) {
      setSelectionError(undefined);
      return;
    }
    setSelectionError(
      canSelectThreadForResume(affordance, selectedThreadId, currentThreadId),
    );
  }, [affordance, currentThreadId, selectedThreadId]);

  return (
    <SessionLifecycleHost
      items={items}
      selectedThreadId={selectedThreadId}
      currentThreadId={currentThreadId}
      phaseLabel={phaseLabel(phase)}
      affordance={affordance}
      selectionError={selectionError}
      listError={listError}
      cancelError={cancelError}
      resumeHint={resumeHint}
      busy={busy}
      loading={loading}
      hasMore={typeof nextOffset === "number"}
      canCancel={canCancel}
      onRefreshThreads={() =>
        void run(async () => {
          listGeneration.current += 1;
          await loadThreads({ offset: 0 });
        })
      }
      onLoadMoreThreads={() =>
        void run(async () => {
          if (typeof nextOffset !== "number") return;
          await loadThreads({ append: true, offset: nextOffset });
        })
      }
      onSelectThread={(threadId) => {
        setResumeHint(undefined);
        setSelectedThreadId(threadId);
      }}
      onCancelTurn={() =>
        void run(async () => {
          setCancelError(undefined);
          if (!canCancel) {
            setCancelError("Cancel requires an active writable controller lease.");
            return;
          }
          try {
            await controller.cancel();
          } catch (cause) {
            setCancelError(errorMessage(cause));
          }
        })
      }
      onResumeIntent={() =>
        void run(async () => {
          setResumeHint(undefined);
          setSelectionError(undefined);
          const threadId = selectedThreadId?.trim() ?? "";
          const blocked = canSelectThreadForResume(affordance, threadId, currentThreadId);
          setSelectionError(blocked);
          if (blocked) return;
          try {
            await controller.withLease((token) => api.resumeSession(threadId, token));
            // In-process swap is not available yet; a 200 would mean the
            // server swapped projection+runtime atomically.
            setResumeHint(`Resumed thread ${threadId}.`);
          } catch (cause) {
            const code =
              cause && typeof cause === "object" && "code" in cause
                ? String((cause as { code?: string }).code ?? "")
                : "";
            const message = errorMessage(cause);
            // Fail-closed restart hint is the expected production path today.
            if (code === "SESSION_RESUME_REQUIRES_RESTART") {
              setResumeHint(message);
              return;
            }
            setSelectionError(message);
            // Keep the server error actionable — do not claim the target is
            // ready after SESSION_RESUME_NOT_FOUND / LOAD_FAILED.
          }
        })
      }
    />
  );
}
