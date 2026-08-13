import { describe, expect, it } from "vitest";

import { queryFixture, sessionFixture } from "../fixtures";

import {
  backlogResyncScenario,
  codeUiV2EventsUrl,
  cursorReconnectScenario,
  envelopeFromFoldedSnapshot,
  foldSseEvents,
  foldWireV2Event,
  initialSseResilienceState,
  isWireV2Resync,
  parseWireV2Event,
  parseWireV2Resync,
  reduceSseResilience,
  reconnectingSseState,
  WIRE_V2_RESYNC_REQUIRED,
} from ".";

describe("sse-resilience helpers", () => {
  it("keeps cursor and retained snapshot across disconnect/reconnect", () => {
    const { start, events } = cursorReconnectScenario();
    expect(start.cursor.lastSeq).toBe(7);
    const disconnected = reduceSseResilience(start, events[0]!);
    expect(disconnected.status).toBe("reconnecting");
    expect(disconnected.snapshotRetained).toBe(true);
    expect(disconnected.cursor.lastSeq).toBe(7);
    const restored = reduceSseResilience(disconnected, events[1]!);
    expect(restored.status).toBe("connected");
    expect(restored.cursor.lastSeq).toBe(8);
  });

  it("covers backlog overflow and one explicit snapshot resync", () => {
    const { start, events } = backlogResyncScenario();
    const end = foldSseEvents(start, events);
    expect(end.status).toBe("resynced");
    expect(end.backlogHint).toBeUndefined();
    expect(end.cursor.lastSeq).toBe(12);
    expect(end.snapshotRetained).toBe(true);
  });

  it("keeps reconnecting after snapshot resync while the stream is still down", () => {
    const disconnected = reduceSseResilience(
      reduceSseResilience(initialSseResilienceState(), { type: "stream_event", seq: 4 }),
      { type: "stream_disconnect", snapshotRetained: true },
    );
    const afterResync = foldSseEvents(disconnected, [
      { type: "resync_requested" },
      { type: "resync_completed", seq: 4 },
    ]);
    expect(afterResync.status).toBe("reconnecting");
    expect(afterResync.snapshotRetained).toBe(true);
    expect(afterResync.cursor.lastSeq).toBe(4);
    const restored = reduceSseResilience(afterResync, { type: "stream_event", seq: 5 });
    expect(restored.status).toBe("connected");
  });

  it("does not invent sequence numbers", () => {
    const withDisconnect = reduceSseResilience(initialSseResilienceState(), {
      type: "stream_disconnect",
      snapshotRetained: false,
    });
    expect(withDisconnect.cursor.lastSeq).toBeUndefined();
    expect(reconnectingSseState().cursor.lastSeq).toBe(4);
  });

  it("keeps a retained cursor across synthetic reconnect replay seq 0", () => {
    const retained = reduceSseResilience(
      reduceSseResilience(initialSseResilienceState(), { type: "stream_event", seq: 7 }),
      { type: "stream_disconnect", snapshotRetained: true },
    );
    const afterReplay = reduceSseResilience(retained, { type: "stream_event", seq: 0 });
    expect(afterReplay.status).toBe("connected");
    expect(afterReplay.cursor.lastSeq).toBe(7);
    expect(afterReplay.acceptCursorReset).toBe(true);
  });

  it("accepts the first nonzero seq after disconnect when the server restarts", () => {
    const retained = reduceSseResilience(
      reduceSseResilience(initialSseResilienceState(), { type: "stream_event", seq: 7 }),
      { type: "stream_disconnect", snapshotRetained: true },
    );
    const afterReplay = reduceSseResilience(retained, { type: "stream_event", seq: 0 });
    const afterRestart = reduceSseResilience(afterReplay, { type: "stream_event", seq: 1 });
    expect(afterRestart.cursor.lastSeq).toBe(1);
    expect(afterRestart.acceptCursorReset).toBe(false);
  });
});

describe("sse v2 wire", () => {
  it("asks for wire=2 and omits a zero/missing cursor", () => {
    expect(codeUiV2EventsUrl()).toBe("/api/code/events?wire=2");
    expect(codeUiV2EventsUrl("/api/code/events", 0)).toBe("/api/code/events?wire=2");
    expect(codeUiV2EventsUrl("/api/code/events", 12)).toBe("/api/code/events?wire=2&cursor=12");
  });

  it("parses v2 events and resync payloads from the wire", () => {
    const event = parseWireV2Event(
      JSON.stringify({
        cursor: 4,
        eventId: "evt-4",
        kind: "code_ui_projection_delta:status",
        at: "2026-07-15T00:00:04.000Z",
        payload: { projection: "status", summary: "status", payload: "thinking" },
      }),
    );
    expect(event?.cursor).toBe(4);
    expect(event?.kind).toBe("code_ui_projection_delta:status");
    expect(parseWireV2Event("{")).toBeUndefined();

    const resync = parseWireV2Resync(
      JSON.stringify({
        code: WIRE_V2_RESYNC_REQUIRED,
        reason: "transport backlog exceeded",
        lastCursor: 8,
        durableTail: 40,
        action: "fetch_snapshot",
      }),
    );
    expect(resync && isWireV2Resync(resync)).toBe(true);
    expect(resync?.durableTail).toBe(40);
  });

  it("folds projection deltas and uses the wire cursor as seq", () => {
    const start = sessionFixture({
      transcript: [queryFixture({ id: "a1", content: "Hello", status: "streaming", streaming: true })],
      plans: [{ id: "p1", status: "completed", steps: [], updatedAt: "2026-07-15T00:00:00.000Z" }],
    });
    const status = foldWireV2Event(start, {
      cursor: 9,
      kind: "code_ui_projection_delta:status",
      at: "2026-07-15T00:00:09.000Z",
      payload: { projection: "status", payload: "thinking" },
    });
    expect(status.status).toBe("thinking");
    const streamed = foldWireV2Event(status, {
      cursor: 10,
      kind: "code_ui_projection_delta:assistant_delta",
      at: "2026-07-15T00:00:10.000Z",
      payload: { payload: { entryId: "a1", delta: " world", updatedAt: "2026-07-15T00:00:10.000Z" } },
    });
    expect(streamed.transcript[0]?.content).toBe("Hello world");
    const completed = foldWireV2Event(streamed, {
      cursor: 11,
      kind: "code_ui_projection_delta:transcript_upsert",
      at: "2026-07-15T00:00:11.000Z",
      payload: {
        payload: {
          ...queryFixture({ id: "a1", content: "Hello world", status: "completed", streaming: false }),
        },
      },
    });
    const ignoredDelta = foldWireV2Event(completed, {
      cursor: 12,
      kind: "code_ui_projection_delta:assistant_delta",
      at: "2026-07-15T00:00:12.000Z",
      payload: { payload: { entryId: "a1", delta: " late", updatedAt: "2026-07-15T00:00:12.000Z" } },
    });
    expect(ignoredDelta.transcript[0]?.content).toBe("Hello world");
    const ignoredPlan = foldWireV2Event(ignoredDelta, {
      cursor: 13,
      kind: "code_ui_projection_delta:plan_upsert",
      at: "2026-07-15T00:00:13.000Z",
      payload: { payload: { id: "p1", status: "in_progress", steps: [], updatedAt: "2026-07-15T00:00:13.000Z" } },
    });
    expect(ignoredPlan.plans[0]?.status).toBe("completed");
    const envelope = envelopeFromFoldedSnapshot(ignoredPlan, 13, "2026-07-15T00:00:13.000Z");
    expect(envelope.seq).toBe(13);
    expect(envelope.type).toBe("session_updated");
    expect(envelope.data.status).toBe("thinking");
  });

  it("clears a stale threadGraph on a null thread_graph delta", () => {
    const start = sessionFixture({
      threadId: "thread-1",
      threadGraph: {
        threadId: "thread-1",
        nodes: [{ depth: 1, kind: "plan", id: "plan-1", label: "Plan 1", tags: ["selected"] }],
      },
      plans: [{ id: "plan-2", title: "New plan", status: "selected", steps: [], updatedAt: "2026-07-15T00:00:00.000Z" }],
    });
    const cleared = foldWireV2Event(start, {
      cursor: 20,
      kind: "code_ui_projection_delta:thread_graph",
      at: "2026-07-15T00:00:20.000Z",
      payload: { projection: "thread_graph", payload: null },
    });
    expect(cleared.threadGraph).toBeUndefined();
    expect(cleared.plans[0]?.id).toBe("plan-2");
  });
});
