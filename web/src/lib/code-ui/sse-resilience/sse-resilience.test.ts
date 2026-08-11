import { describe, expect, it } from "vitest";

import {
  backlogResyncScenario,
  cursorReconnectScenario,
  foldSseEvents,
  initialSseResilienceState,
  reduceSseResilience,
  reconnectingSseState,
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
