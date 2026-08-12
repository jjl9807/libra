"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PropsWithChildren,
} from "react";

import { createCodeUiClient, type CodeUiClient } from "./client";
import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "./types";

export type CodeUiEventHandler = (event: CodeUiEventEnvelope) => void;
export type CodeUiEventRegistration = (register: (type: string, handler: CodeUiEventHandler) => () => void) => void;

interface CodeUiStoreValue {
  client: CodeUiClient;
  snapshot?: CodeUiSessionSnapshot;
  error?: Error;
  /** Load `/api/code/session` into the shared store.
   * - `applied`: this call wrote the snapshot
   * - `superseded`: a live SSE update already owns newer data (store is fine)
   * - `raced`: a newer refresh owns the outcome (caller should retry or fail closed)
   * - `failed`: this call lost and set `error`
   */
  refresh(): Promise<"applied" | "superseded" | "raced" | "failed">;
  subscribe(type: string, handler: CodeUiEventHandler): () => void;
}

const CodeUiStoreContext = createContext<CodeUiStoreValue | undefined>(undefined);

function timestampMs(timestamp: string): number {
  const parsed = Date.parse(timestamp);
  return Number.isNaN(parsed) ? Number.NEGATIVE_INFINITY : parsed;
}

export function CodeUiStoreProvider({
  children,
  client: injectedClient,
  extensions = [],
  reconnectDelayMs = 1_000,
  maxReconnectDelayMs = 30_000,
}: PropsWithChildren<{
  client?: CodeUiClient;
  extensions?: CodeUiEventRegistration[];
  reconnectDelayMs?: number;
  maxReconnectDelayMs?: number;
}>) {
  const defaultClient = useRef<CodeUiClient | undefined>(undefined);
  const client = injectedClient ?? (defaultClient.current ??= createCodeUiClient());
  const [snapshot, setSnapshot] = useState<CodeUiSessionSnapshot>();
  const [error, setError] = useState<Error>();
  const handlers = useRef(new Map<string, Set<CodeUiEventHandler>>());
  const extensionRegistrations = useRef(extensions);
  const eventGeneration = useRef(0);
  const refreshGeneration = useRef(0);
  const latestSnapshotUpdatedAtMs = useRef<number | undefined>(undefined);

  const subscribe = useCallback((type: string, handler: CodeUiEventHandler) => {
    const set = handlers.current.get(type) ?? new Set<CodeUiEventHandler>();
    set.add(handler);
    handlers.current.set(type, set);
    return () => set.delete(handler);
  }, []);

  const refresh = useCallback(async (): Promise<"applied" | "superseded" | "raced" | "failed"> => {
    const requestedAtGeneration = eventGeneration.current;
    const requestGeneration = ++refreshGeneration.current;
    try {
      const nextSnapshot = await client.snapshot();
      const nextSnapshotUpdatedAtMs = timestampMs(nextSnapshot.updatedAt);
      const receivedNewerSnapshot =
        nextSnapshotUpdatedAtMs > (latestSnapshotUpdatedAtMs.current ?? Number.NEGATIVE_INFINITY);
      const isCurrentRequest = requestGeneration === refreshGeneration.current;
      if (!isCurrentRequest) {
        // A later refresh() owns the outcome — do not treat this as a healthy supersession.
        return "raced";
      }
      if (eventGeneration.current === requestedAtGeneration || receivedNewerSnapshot) {
        latestSnapshotUpdatedAtMs.current = nextSnapshotUpdatedAtMs;
        setSnapshot(nextSnapshot);
        setError(undefined);
        return "applied";
      }
      // Live SSE advanced while we fetched; the store already holds that update.
      return "superseded";
    } catch (cause) {
      const isCurrentRequest = requestGeneration === refreshGeneration.current;
      if (!isCurrentRequest) {
        // A later refresh() owns the outcome — do not clobber its error/snapshot state.
        return "raced";
      }
      const hasSnapshot = latestSnapshotUpdatedAtMs.current !== undefined;
      const sseUnchanged = eventGeneration.current === requestedAtGeneration;
      if (sseUnchanged || !hasSnapshot) {
        setError(cause instanceof Error ? cause : new Error("Unable to load Code UI session"));
        return "failed";
      }
      // SSE advanced while this request failed; treat as raced against live updates.
      return "raced";
    }
  }, [client]);

  useEffect(() => {
    const bootstrap = setTimeout(() => void refresh(), 0);
    let stream: ReturnType<CodeUiClient["observe"]> | undefined;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;
    let reconnectAttempts = 0;
    let connectionGeneration = 0;
    const scheduleReconnect = () => {
      const delay = Math.min(reconnectDelayMs * 2 ** reconnectAttempts, maxReconnectDelayMs);
      reconnectAttempts += 1;
      retry = setTimeout(() => {
        void refresh();
        connect();
      }, delay);
    };
    const connect = () => {
      if (disposed) return;
      const currentConnection = ++connectionGeneration;
      stream = client.observe(
        (event) => {
          if (disposed || currentConnection !== connectionGeneration) return;
          eventGeneration.current += 1;
          reconnectAttempts = 0;
          if (
            "sessionId" in event.data &&
            timestampMs(event.data.updatedAt) >=
              (latestSnapshotUpdatedAtMs.current ?? Number.NEGATIVE_INFINITY)
          ) {
            latestSnapshotUpdatedAtMs.current = timestampMs(event.data.updatedAt);
            setSnapshot(event.data);
            setError(undefined);
          }
          handlers.current.get(event.type)?.forEach((handler) => handler(event));
          handlers.current.get("*")?.forEach((handler) => handler(event));
        },
        () => {
          if (disposed || currentConnection !== connectionGeneration) return;
          stream?.close();
          scheduleReconnect();
        },
      );
    };
    extensionRegistrations.current.forEach((extension) => extension(subscribe));
    connect();
    return () => {
      disposed = true;
      stream?.close();
      clearTimeout(bootstrap);
      if (retry) clearTimeout(retry);
    };
  }, [client, maxReconnectDelayMs, reconnectDelayMs, refresh, subscribe]);

  const value = useMemo(
    () => ({ client, snapshot, error, refresh, subscribe }),
    [client, error, refresh, snapshot, subscribe],
  );
  return <CodeUiStoreContext.Provider value={value}>{children}</CodeUiStoreContext.Provider>;
}

export function useCodeUiStore(): CodeUiStoreValue {
  const value = useContext(CodeUiStoreContext);
  if (!value) throw new Error("useCodeUiStore must be used inside CodeUiStoreProvider");
  return value;
}
