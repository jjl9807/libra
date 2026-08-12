import type { CodeUiClient, CodeUiEventStream } from "../client";
import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "../types";

import {
  codeUiV2EventsUrl,
  envelopeFromFoldedSnapshot,
  foldWireV2Event,
  isWireV2Resync,
  parseWireV2Event,
  parseWireV2Resync,
  type CodeUiWireV2Event,
  type CodeUiWireV2ResyncEvent,
} from "./v2-wire";

type DisconnectListener = () => void;
type ResyncListener = (event: CodeUiWireV2ResyncEvent) => void;

type ResyncCompleteListener = (seq?: number) => void;

const disconnectListeners = new Set<DisconnectListener>();
const resyncListeners = new Set<ResyncListener>();
const resyncCompleteListeners = new Set<ResyncCompleteListener>();

type SnapshotGate = {
  allowOnce: boolean;
  cached: CodeUiSessionSnapshot | undefined;
  lastCursor: number | undefined;
  pending: CodeUiWireV2Event[];
  holdReconnect: boolean;
  pendingOnError?: () => void;
  startOnCached?: () => void;
  bootstrapOnError?: () => void;
  useV1: boolean;
};

let activeSnapshotGate: SnapshotGate | undefined;

function notifyDisconnect(): void {
  disconnectListeners.forEach((listener) => listener());
}

function notifyResync(event: CodeUiWireV2ResyncEvent): void {
  resyncListeners.forEach((listener) => listener(event));
}

function notifyResyncComplete(seq?: number): void {
  resyncCompleteListeners.forEach((listener) => listener(seq));
}

function shouldUseNativeV2Observe(): boolean {
  return (
    typeof fetch === "function" &&
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT !== true
  );
}

function releaseReconnectHold(gate: SnapshotGate): void {
  if (!gate.holdReconnect) return;
  gate.holdReconnect = false;
  const pending = gate.pendingOnError;
  gate.pendingOnError = undefined;
  pending?.();
}

function isAssistantDeltaKind(kind: string): boolean {
  return kind === "code_ui_projection_delta:assistant_delta";
}

function isLiveRelativeToSnapshot(snapshot: CodeUiSessionSnapshot, at: string): boolean {
  const eventAt = Date.parse(at);
  const snapshotAt = Date.parse(snapshot.updatedAt);
  if (Number.isNaN(eventAt) || Number.isNaN(snapshotAt)) return true;
  return eventAt > snapshotAt;
}

/** Subscribe to SSE disconnects observed through a wrapped client. */
export function subscribeSseDisconnect(listener: DisconnectListener): () => void {
  disconnectListeners.add(listener);
  return () => {
    disconnectListeners.delete(listener);
  };
}

/** Subscribe to W3-08 `event: resync` payloads (WIRE_V2_RESYNC_REQUIRED). */
export function subscribeSseResync(listener: ResyncListener): () => void {
  resyncListeners.add(listener);
  return () => {
    resyncListeners.delete(listener);
  };
}

/** Subscribe to successful automatic snapshot resync (W3-08 / ahead-cursor). */
export function subscribeSseResyncComplete(listener: ResyncCompleteListener): () => void {
  resyncCompleteListeners.add(listener);
  return () => {
    resyncCompleteListeners.delete(listener);
  };
}

/**
 * Allow the next `snapshot()` call to hit the network. Used for bootstrap and
 * explicit W3-08 resync; reconnect catch-up uses the v2 cursor instead.
 */
export function allowSseSnapshotFetch(): void {
  if (activeSnapshotGate) activeSnapshotGate.allowOnce = true;
}

/**
 * Wrap a Code UI client so the built-in SPA consumes SSE wire v2 (cursor +
 * resync) without modifying W2-07 `store.tsx` / `client.ts`.
 *
 * Reconnect opens `GET /api/code/events?wire=2&cursor=<last>` and does not
 * pull a full snapshot. Backlog overflow / ahead-cursor fetch one snapshot,
 * then reconnect at `durableTail` (or from 0 after dropping an ahead cursor).
 */
export function wrapClientForSseResilience(client: CodeUiClient): CodeUiClient {
  const gate: SnapshotGate = {
    allowOnce: true,
    cached: undefined,
    lastCursor: undefined,
    pending: [],
    holdReconnect: false,
    useV1: false,
  };
  activeSnapshotGate = gate;

  return {
    ...client,
    async snapshot() {
      if (!gate.allowOnce && gate.cached) {
        applyPending(gate);
        return gate.cached;
      }
      try {
        const next = await client.snapshot();
        gate.cached = next;
        gate.allowOnce = false;
        applyPending(gate);
        releaseReconnectHold(gate);
        const start = gate.startOnCached;
        gate.startOnCached = undefined;
        gate.bootstrapOnError = undefined;
        start?.();
        return gate.cached ?? next;
      } catch (error) {
        const onError = gate.bootstrapOnError;
        gate.startOnCached = undefined;
        gate.bootstrapOnError = undefined;
        if (onError) {
          notifyDisconnect();
          onError();
        }
        throw error;
      }
    },
    observe(onEvent, onError): CodeUiEventStream {
      if (!shouldUseNativeV2Observe() || gate.useV1) {
        return client.observe(onEvent, () => {
          notifyDisconnect();
          onError();
        });
      }
      const abort = new AbortController();
      const start = () => {
        if (abort.signal.aborted || gate.holdReconnect) return;
        void runV2FetchStream(gate, client, onEvent, onError, abort);
      };
      if (gate.cached && !gate.holdReconnect) {
        start();
      } else {
        const previous = gate.startOnCached;
        gate.bootstrapOnError = onError;
        gate.startOnCached = () => {
          previous?.();
          start();
        };
      }
      return {
        close: () => abort.abort(),
      };
    },
  };
}

function applyPending(gate: SnapshotGate): void {
  if (!gate.cached) return;
  // HTTP snapshot already includes buffered history. Skip assistant_delta so
  // bootstrap cannot duplicate streamed text; upserts are last-write-wins.
  for (const event of gate.pending.splice(0)) {
    if (isAssistantDeltaKind(event.kind)) continue;
    gate.cached = foldWireV2Event(gate.cached, event);
  }
}

async function runV2FetchStream(
  gate: SnapshotGate,
  inner: CodeUiClient,
  onEvent: (event: CodeUiEventEnvelope) => void,
  onError: () => void,
  abort: AbortController,
): Promise<void> {
  const url = codeUiV2EventsUrl("/api/code/events", gate.lastCursor);
  let closed = false;
  const close = () => {
    closed = true;
    abort.abort();
  };

  const handleWorkflow = (raw: string) => {
    const parsed = parseWireV2Event(raw);
    if (!parsed) return;
    gate.lastCursor = parsed.cursor;
    if (!gate.cached) {
      gate.pending.push(parsed);
      return;
    }
    if (isAssistantDeltaKind(parsed.kind) && !isLiveRelativeToSnapshot(gate.cached, parsed.at)) {
      return;
    }
    const folded = foldWireV2Event(gate.cached, parsed);
    gate.cached = folded;
    onEvent(envelopeFromFoldedSnapshot(folded, parsed.cursor, parsed.at));
  };

  const handleResync = (raw: string) => {
    const parsed = parseWireV2Resync(raw);
    if (!parsed || !isWireV2Resync(parsed)) return;
    beginSnapshotResync(gate, inner, onError, parsed);
    close();
  };

  try {
    const response = await fetch(url, {
      credentials: "same-origin",
      headers: { accept: "text/event-stream" },
      signal: abort.signal,
    });
    if (closed || abort.signal.aborted) return;
    if (response.status === 409) {
      beginSnapshotResync(gate, inner, onError, {
        code: "WIRE_V2_CURSOR_AHEAD",
        reason: "SSE cursor is ahead of the durable tail; fetch a snapshot and reconnect.",
        lastCursor: gate.lastCursor ?? 0,
        durableTail: 0,
        action: "fetch_snapshot",
      });
      gate.lastCursor = undefined;
      return;
    }
    if (response.status === 503) {
      const code = await readErrorCode(response);
      if (code === "WIRE_V2_REQUIRES_DURABLE_SESSION") {
        gate.useV1 = true;
        const fallback = inner.observe(onEvent, () => {
          notifyDisconnect();
          onError();
        });
        abort.signal.addEventListener("abort", () => fallback.close(), { once: true });
        return;
      }
    }
    if (!response.ok || !response.body) {
      notifyDisconnect();
      onError();
      return;
    }
    await consumeSse(response.body, abort.signal, (event, data) => {
      if (event === "code_workflow") handleWorkflow(data);
      else if (event === "resync") handleResync(data);
    });
    if (!closed && !abort.signal.aborted) {
      notifyDisconnect();
      onError();
    }
  } catch (cause) {
    if (closed || abort.signal.aborted || (cause as { name?: string }).name === "AbortError") {
      return;
    }
    notifyDisconnect();
    onError();
  }
}

function beginSnapshotResync(
  gate: SnapshotGate,
  inner: CodeUiClient,
  onError: () => void,
  event: CodeUiWireV2ResyncEvent,
): void {
  if (event.code === "WIRE_V2_RESYNC_REQUIRED") {
    gate.lastCursor = event.durableTail;
  }
  gate.holdReconnect = true;
  gate.pendingOnError = onError;
  gate.allowOnce = true;
  notifyResync(event);
  void inner
    .snapshot()
    .then((next) => {
      gate.cached = next;
      gate.allowOnce = false;
      applyPending(gate);
      releaseReconnectHold(gate);
      notifyResyncComplete(gate.lastCursor);
    })
    .catch(() => {
      // Keep allowOnce so the W2-15 Resync button can retry the snapshot pull.
    });
}

async function readErrorCode(response: Response): Promise<string | undefined> {
  try {
    const body = (await response.json()) as { error?: { code?: string } };
    return typeof body.error?.code === "string" ? body.error.code : undefined;
  } catch {
    return undefined;
  }
}

async function consumeSse(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
  onFrame: (event: string, data: string) => void,
): Promise<void> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  while (!signal.aborted) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
    let split = buffer.indexOf("\n\n");
    while (split >= 0) {
      const frame = buffer.slice(0, split);
      buffer = buffer.slice(split + 2);
      let eventType = "message";
      const dataLines: string[] = [];
      for (const line of frame.split("\n")) {
        if (line.startsWith("event:")) eventType = line.slice(6).trim();
        else if (line.startsWith("data:")) dataLines.push(line.slice(5).trimStart());
      }
      if (dataLines.length > 0) onFrame(eventType, dataLines.join("\n"));
      split = buffer.indexOf("\n\n");
    }
  }
}
