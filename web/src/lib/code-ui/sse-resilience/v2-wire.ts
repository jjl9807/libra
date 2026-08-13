import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "../types";

export const WIRE_V2_RESYNC_REQUIRED = "WIRE_V2_RESYNC_REQUIRED";

/** Minimal v2 SSE payload (camelCase). Cursor is the durable workflow sequence. */
export interface CodeUiWireV2Event {
  cursor: number;
  eventId?: string;
  kind: string;
  at: string;
  payload?: unknown;
}

export interface CodeUiWireV2ResyncEvent {
  code: string;
  reason: string;
  lastCursor: number;
  durableTail: number;
  action: string;
}

export function codeUiV2EventsUrl(basePath = "/api/code/events", cursor?: number): string {
  const params = new URLSearchParams({ wire: "2" });
  if (typeof cursor === "number" && Number.isFinite(cursor) && cursor > 0) {
    params.set("cursor", String(Math.trunc(cursor)));
  }
  return `${basePath}?${params.toString()}`;
}

export function parseWireV2Event(raw: string): CodeUiWireV2Event | undefined {
  try {
    const value = JSON.parse(raw) as Partial<CodeUiWireV2Event>;
    if (typeof value.cursor !== "number" || typeof value.kind !== "string") return undefined;
    return {
      cursor: value.cursor,
      eventId: typeof value.eventId === "string" ? value.eventId : undefined,
      kind: value.kind,
      at: typeof value.at === "string" ? value.at : new Date(0).toISOString(),
      payload: value.payload,
    };
  } catch {
    return undefined;
  }
}

export function parseWireV2Resync(raw: string): CodeUiWireV2ResyncEvent | undefined {
  try {
    const value = JSON.parse(raw) as Partial<CodeUiWireV2ResyncEvent>;
    if (typeof value.code !== "string" || typeof value.durableTail !== "number") return undefined;
    return {
      code: value.code,
      reason: typeof value.reason === "string" ? value.reason : "transport backlog exceeded",
      lastCursor: typeof value.lastCursor === "number" ? value.lastCursor : 0,
      durableTail: value.durableTail,
      action: typeof value.action === "string" ? value.action : "fetch_snapshot",
    };
  } catch {
    return undefined;
  }
}

export function isWireV2Resync(event: CodeUiWireV2ResyncEvent): boolean {
  return event.code === WIRE_V2_RESYNC_REQUIRED;
}

/**
 * Fold a v2 workflow event onto a bootstrap snapshot. Unknown kinds are
 * ignored (forward compatible). Sequence numbers are never invented here —
 * callers pass `event.cursor` through to the store envelope.
 */
export function foldWireV2Event(
  snapshot: CodeUiSessionSnapshot,
  event: CodeUiWireV2Event,
): CodeUiSessionSnapshot {
  const prefix = "code_ui_projection_delta:";
  if (!event.kind.startsWith(prefix)) {
    return { ...snapshot, updatedAt: event.at || snapshot.updatedAt };
  }
  const projection = event.kind.slice(prefix.length);
  const wrapper = asRecord(event.payload);
  const payload = wrapper && "payload" in wrapper ? wrapper.payload : event.payload;
  return applyProjectionDelta(snapshot, projection, payload, event.at);
}

export function envelopeFromFoldedSnapshot(
  snapshot: CodeUiSessionSnapshot,
  cursor: number,
  at: string,
): CodeUiEventEnvelope {
  return {
    seq: cursor,
    type: "session_updated",
    at,
    data: snapshot,
  };
}

function applyProjectionDelta(
  snapshot: CodeUiSessionSnapshot,
  projection: string,
  payload: unknown,
  at: string,
): CodeUiSessionSnapshot {
  const next: CodeUiSessionSnapshot = { ...snapshot, updatedAt: at || snapshot.updatedAt };
  switch (projection) {
    case "status":
      if (typeof payload === "string") next.status = payload as CodeUiSessionSnapshot["status"];
      return next;
    case "controller":
      if (payload && typeof payload === "object") {
        next.controller = payload as CodeUiSessionSnapshot["controller"];
      }
      return next;
    case "plan_execution_repair":
      next.planExecutionRepair = payload
        ? (payload as CodeUiSessionSnapshot["planExecutionRepair"])
        : undefined;
      return next;
    case "transcript_upsert":
      next.transcript = upsertById(snapshot.transcript, payload);
      return next;
    case "assistant_delta": {
      const delta = asRecord(payload);
      const entryId = typeof delta?.entryId === "string" ? delta.entryId : undefined;
      const text = typeof delta?.delta === "string" ? delta.delta : "";
      const updatedAt = typeof delta?.updatedAt === "string" ? delta.updatedAt : at;
      if (!entryId || !text) return next;
      next.transcript = snapshot.transcript.map((entry) => {
        if (entry.id !== entryId) return entry;
        if (entry.status === "completed" || entry.status === "error" || entry.status === "cancelled") {
          return entry;
        }
        return {
          ...entry,
          content: `${entry.content ?? ""}${text}`,
          streaming: true,
          updatedAt,
        };
      });
      return next;
    }
    case "interaction_upsert":
      next.interactions = upsertById(snapshot.interactions, payload);
      return next;
    case "interaction_resolved": {
      const resolution = asRecord(payload);
      const id = typeof resolution?.interactionId === "string" ? resolution.interactionId : undefined;
      if (!id) return next;
      const resolvedAt = typeof resolution?.resolvedAt === "string" ? resolution.resolvedAt : undefined;
      next.interactions = snapshot.interactions.map((item) =>
        item.id === id
          ? {
              ...item,
              status: "resolved",
              resolvedAt: resolvedAt ?? item.resolvedAt,
            }
          : item,
      );
      return next;
    }
    case "interaction_cleared": {
      const clear = asRecord(payload);
      const id = typeof clear?.interactionId === "string" ? clear.interactionId : undefined;
      if (!id) return next;
      next.interactions = snapshot.interactions.filter((item) => item.id !== id);
      return next;
    }
    case "plan_upsert": {
      const plan = asRecord(payload);
      const id = typeof plan?.id === "string" ? plan.id : undefined;
      const incomingStatus = typeof plan?.status === "string" ? plan.status : "";
      if (id) {
        const existing = snapshot.plans.find((item) => item.id === id);
        if (
          existing &&
          isTerminalPlanStatus(existing.status) &&
          !isTerminalPlanStatus(incomingStatus)
        ) {
          return next;
        }
      }
      next.plans = upsertById(snapshot.plans, payload);
      return next;
    }
    case "task_upsert":
      next.tasks = upsertById(snapshot.tasks, payload);
      return next;
    case "tool_call_upsert":
      next.toolCalls = upsertById(snapshot.toolCalls, payload);
      return next;
    case "patchset_upsert":
      next.patchsets = upsertById(snapshot.patchsets, payload);
      return next;
    case "thread_graph":
      next.threadGraph = payload
        ? (payload as CodeUiSessionSnapshot["threadGraph"])
        : undefined;
      return next;
    default:
      return next;
  }
}

function isTerminalPlanStatus(status: string): boolean {
  return status === "completed" || status === "failed";
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  return value as Record<string, unknown>;
}

function upsertById<T extends { id: string }>(items: T[], incoming: unknown): T[] {
  const record = asRecord(incoming);
  const id = typeof record?.id === "string" ? record.id : undefined;
  if (!id) return items;
  const next = incoming as T;
  const index = items.findIndex((item) => item.id === id);
  if (index < 0) return [...items, next];
  const copy = items.slice();
  copy[index] = next;
  return copy;
}
