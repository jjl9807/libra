import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../client";
import { queryFixture, sessionFixture } from "../fixtures";
import type { CodeUiEventEnvelope } from "../types";

import {
  allowSseSnapshotFetch,
  codeUiV2EventsUrl,
  subscribeSseDisconnect,
  subscribeSseResync,
  subscribeSseResyncComplete,
  WIRE_V2_RESYNC_REQUIRED,
  wrapClientForSseResilience,
} from ".";

class FakeV2Fetch {
  urls: string[] = [];
  aheadCursors = new Set<number>();
  hubMissing = false;
  private readonly writers: Array<WritableStreamDefaultWriter<Uint8Array>> = [];
  private readonly encoder = new TextEncoder();

  install(): void {
    vi.stubGlobal("fetch", async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      this.urls.push(url);
      const cursor = Number(new URL(url, "http://127.0.0.1").searchParams.get("cursor") ?? "0");
      if (this.hubMissing) {
        return new Response(
          JSON.stringify({
            error: { code: "WIRE_V2_REQUIRES_DURABLE_SESSION", message: "no hub" },
          }),
          { status: 503 },
        );
      }
      if (cursor > 0 && this.aheadCursors.has(cursor)) {
        return new Response(JSON.stringify({ error: { code: "WIRE_V2_CURSOR_AHEAD", message: "ahead" } }), {
          status: 409,
        });
      }
      const { readable, writable } = new TransformStream<Uint8Array, Uint8Array>();
      this.writers.push(writable.getWriter());
      return new Response(readable, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    });
  }

  async end(): Promise<void> {
    await vi.waitFor(() => expect(this.writers.length).toBeGreaterThan(0));
    await this.writers.at(-1)!.close();
  }

  async emit(event: string, data: unknown): Promise<void> {
    await vi.waitFor(() => expect(this.writers.length).toBeGreaterThan(0));
    const payload = typeof data === "string" ? data : JSON.stringify(data);
    await this.writers.at(-1)!.write(this.encoder.encode(`event: ${event}\ndata: ${payload}\n\n`));
  }
}

describe("sse v2 stream lifecycle", () => {
  let fake: FakeV2Fetch;

  beforeEach(() => {
    fake = new FakeV2Fetch();
    fake.install();
    delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("consumes v2 deltas and reconnects with cursor without another snapshot fetch", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    const snapshot = vi.spyOn(inner, "snapshot");
    const client = wrapClientForSseResilience(inner);
    const events: CodeUiEventEnvelope[] = [];

    await client.snapshot();
    expect(snapshot).toHaveBeenCalledTimes(1);

    const first = client.observe((event) => events.push(event), vi.fn());
    await vi.waitFor(() => expect(fake.urls[0]).toBe(codeUiV2EventsUrl()));
    await fake.emit("code_workflow", {
      cursor: 8,
      kind: "code_ui_projection_delta:status",
      at: "2026-07-15T00:00:08.000Z",
      payload: { projection: "status", payload: "thinking" },
    });
    await vi.waitFor(() => expect(events[0]?.seq).toBe(8));
    expect(events[0]?.data.status).toBe("thinking");

    first.close();
    await client.snapshot();
    expect(snapshot).toHaveBeenCalledTimes(1);

    client.observe(vi.fn(), vi.fn());
    await vi.waitFor(() =>
      expect(fake.urls[1]).toBe(codeUiV2EventsUrl("/api/code/events", 8)),
    );
  });

  it("keeps the cursor when a handshake fails before any event", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    const client = wrapClientForSseResilience(inner);
    await client.snapshot();
    const first = client.observe(vi.fn(), vi.fn());
    await fake.emit("code_workflow", {
      cursor: 8,
      kind: "code_ui_projection_delta:status",
      at: "2026-07-15T00:00:08.000Z",
      payload: { projection: "status", payload: "thinking" },
    });
    first.close();
    const onError = vi.fn();
    client.observe(vi.fn(), onError);
    await vi.waitFor(() =>
      expect(fake.urls[1]).toBe(codeUiV2EventsUrl("/api/code/events", 8)),
    );
    await fake.end();
    await vi.waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
    client.observe(vi.fn(), vi.fn());
    await vi.waitFor(() =>
      expect(fake.urls[2]).toBe(codeUiV2EventsUrl("/api/code/events", 8)),
    );
  });

  it("skips historical assistant_delta but applies live deltas after bootstrap", async () => {
    const inner = new MockCodeUiClient(
      sessionFixture({
        transcript: [queryFixture({ id: "a1", content: "Hello", status: "streaming", streaming: true })],
      }),
    );
    const client = wrapClientForSseResilience(inner);
    const events: CodeUiEventEnvelope[] = [];
    await client.snapshot();
    client.observe((event) => events.push(event), vi.fn());
    await fake.emit("code_workflow", {
      cursor: 2,
      kind: "code_ui_projection_delta:assistant_delta",
      at: "2026-07-15T00:00:00.000Z",
      payload: { payload: { entryId: "a1", delta: " ignored", updatedAt: "2026-07-15T00:00:00.000Z" } },
    });
    await fake.emit("code_workflow", {
      cursor: 3,
      kind: "code_ui_projection_delta:assistant_delta",
      at: "2026-07-15T00:00:05.000Z",
      payload: { payload: { entryId: "a1", delta: " world", updatedAt: "2026-07-15T00:00:05.000Z" } },
    });
    await fake.emit("code_workflow", {
      cursor: 4,
      kind: "code_ui_projection_delta:status",
      at: "2026-07-15T00:00:06.000Z",
      payload: { projection: "status", payload: "thinking" },
    });
    await vi.waitFor(() => expect(events.some((event) => event.data.status === "thinking")).toBe(true));
    const latest = events.at(-1);
    expect(latest?.data.transcript[0]?.content).toBe("Hello world");
  });

  it("surfaces WIRE_V2_RESYNC_REQUIRED and reconnects only after a snapshot pull", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    const snapshot = vi.spyOn(inner, "snapshot");
    const client = wrapClientForSseResilience(inner);
    const resyncs: unknown[] = [];
    const completed: Array<number | undefined> = [];
    const unsubscribe = subscribeSseResync((event) => resyncs.push(event));
    const unsubscribeComplete = subscribeSseResyncComplete((seq) => completed.push(seq));
    const onError = vi.fn();

    await client.snapshot();
    client.observe(vi.fn(), onError);
    await fake.emit("code_workflow", {
      cursor: 8,
      kind: "code_ui_projection_delta:status",
      at: "2026-07-15T00:00:08.000Z",
      payload: { projection: "status", payload: "thinking" },
    });
    inner.currentSnapshot = sessionFixture({ status: "completed" });
    await fake.emit("resync", {
      code: WIRE_V2_RESYNC_REQUIRED,
      reason: "SSE backlog exceeded; request an explicit snapshot resync.",
      lastCursor: 8,
      durableTail: 40,
      action: "fetch_snapshot",
    });

    await vi.waitFor(() => expect(resyncs.length).toBe(1));
    expect(resyncs[0]).toMatchObject({
      code: WIRE_V2_RESYNC_REQUIRED,
      durableTail: 40,
      action: "fetch_snapshot",
    });
    await vi.waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
    expect(completed).toEqual([40]);
    expect(snapshot).toHaveBeenCalledTimes(2);
    expect(await client.snapshot()).toMatchObject({ status: "completed" });
    expect(snapshot).toHaveBeenCalledTimes(2);

    client.observe(vi.fn(), vi.fn());
    await vi.waitFor(() =>
      expect(fake.urls.some((url) => url === codeUiV2EventsUrl("/api/code/events", 40))).toBe(true),
    );
    unsubscribe();
    unsubscribeComplete();
  });

  it("keeps the reconnect hold when the resync snapshot pull fails until a retry", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    const snapshot = vi.spyOn(inner, "snapshot");
    const client = wrapClientForSseResilience(inner);
    const onError = vi.fn();

    await client.snapshot();
    snapshot.mockRejectedValueOnce(new Error("session unavailable"));
    client.observe(vi.fn(), onError);
    await fake.emit("resync", {
      code: WIRE_V2_RESYNC_REQUIRED,
      reason: "SSE backlog exceeded; request an explicit snapshot resync.",
      lastCursor: 8,
      durableTail: 40,
      action: "fetch_snapshot",
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(onError).not.toHaveBeenCalled();

    allowSseSnapshotFetch();
    inner.currentSnapshot = sessionFixture({ status: "completed" });
    await expect(client.snapshot()).resolves.toMatchObject({ status: "completed" });
    expect(onError).toHaveBeenCalledTimes(1);
    client.observe(vi.fn(), vi.fn());
    await vi.waitFor(() =>
      expect(fake.urls.some((url) => url === codeUiV2EventsUrl("/api/code/events", 40))).toBe(true),
    );
  });

  it("drops an ahead cursor, fetches a snapshot, and reconnects from 0", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    const snapshot = vi.spyOn(inner, "snapshot");
    const client = wrapClientForSseResilience(inner);
    const onError = vi.fn();
    const resyncs: unknown[] = [];
    const unsubscribe = subscribeSseResync((event) => resyncs.push(event));

    await client.snapshot();
    const first = client.observe(vi.fn(), vi.fn());
    await fake.emit("code_workflow", {
      cursor: 8,
      kind: "code_ui_projection_delta:status",
      at: "2026-07-15T00:00:08.000Z",
      payload: { projection: "status", payload: "thinking" },
    });
    first.close();
    fake.aheadCursors.add(8);
    inner.currentSnapshot = sessionFixture({ status: "idle" });
    client.observe(vi.fn(), onError);
    await vi.waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
    expect(resyncs[0]).toMatchObject({ code: "WIRE_V2_CURSOR_AHEAD", action: "fetch_snapshot" });
    expect(snapshot).toHaveBeenCalledTimes(2);

    client.observe(vi.fn(), vi.fn());
    await vi.waitFor(() => expect(fake.urls.at(-1)).toBe(codeUiV2EventsUrl()));
    unsubscribe();
  });

  it("falls back to v1 observe when the runtime has no workflow hub", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    const observe = vi.spyOn(inner, "observe");
    const client = wrapClientForSseResilience(inner);
    fake.hubMissing = true;
    await client.snapshot();
    const events: CodeUiEventEnvelope[] = [];
    client.observe((event) => events.push(event), vi.fn());
    await vi.waitFor(() => expect(observe).toHaveBeenCalled());
    inner.emit({
      seq: 2,
      type: "session_updated",
      at: "2026-07-15T00:00:02.000Z",
      data: sessionFixture({ status: "thinking" }),
    });
    expect(events[0]?.data.status).toBe("thinking");
  });

  it("invokes observe onError after a failed bootstrap snapshot so reconnect can retry", async () => {
    const inner = new MockCodeUiClient(sessionFixture());
    vi.spyOn(inner, "snapshot").mockRejectedValueOnce(new Error("unavailable"));
    const client = wrapClientForSseResilience(inner);
    const disconnected = vi.fn();
    const onError = vi.fn();
    const unsubscribe = subscribeSseDisconnect(disconnected);
    client.observe(vi.fn(), onError);
    await expect(client.snapshot()).rejects.toThrow("unavailable");
    expect(disconnected).toHaveBeenCalled();
    expect(onError).toHaveBeenCalled();
    unsubscribe();
  });
});
