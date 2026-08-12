"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  createUsageApi,
  isAbsentUsageError,
  type UsageApi,
  type UsageReadModel,
} from "@/lib/code-ui/usage";
import { useCodeUiStore } from "@/lib/code-ui/store";

import { UsageHost } from "./UsageHost";

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Request failed. Try again.";
}

export interface SessionUsageProps {
  /** Test seam — production leaves this unset and uses same-origin fetch. */
  api?: UsageApi;
  /** Optional injected read model for fixture-driven demos/tests. */
  initialModel?: UsageReadModel;
}

/**
 * Displays W2-12 usage read model totals. Live `/api/code/usage` is W3-01;
 * until then absent HTTP surfaces an explicit empty/deferred state (not zeros).
 *
 * Snapshot has sessionId/threadId but no runtime turnId yet — never pass
 * threadId as turnId. Current-turn delta arrives in the read model body when
 * the future wire includes it.
 */
export function SessionUsage({ api: injectedApi, initialModel }: SessionUsageProps) {
  const { snapshot } = useCodeUiStore();
  const api = useMemo(() => injectedApi ?? createUsageApi(), [injectedApi]);
  const sessionId = snapshot?.sessionId;
  const threadId = snapshot?.threadId;
  const [model, setModel] = useState<UsageReadModel | undefined>(initialModel);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [deferredHint, setDeferredHint] = useState<string>();
  const generation = useRef(0);
  const scopeKey = `${sessionId ?? ""}::${threadId ?? ""}`;
  const lastScopeKey = useRef(scopeKey);

  const refresh = useCallback(async () => {
    if (!sessionId) return;
    const requestGeneration = ++generation.current;
    setBusy(true);
    setError(undefined);
    try {
      const next = await api.fetchReadModel({
        sessionId,
        threadId,
        // Do not invent turnId from threadId — they are distinct filters.
      });
      if (requestGeneration !== generation.current) return;
      setModel(next);
      setDeferredHint(undefined);
    } catch (cause) {
      if (requestGeneration !== generation.current) return;
      if (isAbsentUsageError(cause)) {
        setModel(undefined);
        setDeferredHint(
          "Usage query HTTP lands in W3-01. Until then this panel stays empty unless a fixture/injected transport supplies the W2-12 read model.",
        );
        return;
      }
      setError(errorMessage(cause));
    } finally {
      // Only the latest generation owns the busy flag — stale hangers cannot
      // leave Refresh permanently disabled.
      if (requestGeneration === generation.current) {
        setBusy(false);
      }
    }
  }, [api, sessionId, threadId]);

  useEffect(() => {
    if (initialModel) return;
    if (!sessionId) return;
    if (lastScopeKey.current !== scopeKey) {
      lastScopeKey.current = scopeKey;
      // Drop prior-scope totals immediately so a slow refetch cannot leave
      // another session's usage visible.
      setModel(undefined);
      setError(undefined);
      setDeferredHint(undefined);
    }
    void refresh();
  }, [initialModel, refresh, scopeKey, sessionId]);

  return (
    <UsageHost
      model={model}
      busy={busy}
      error={error}
      deferredHint={deferredHint}
      onRefresh={() => void refresh()}
    />
  );
}
