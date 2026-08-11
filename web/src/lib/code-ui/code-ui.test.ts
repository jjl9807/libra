// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  createCodeUiClient,
  MockCodeUiClient,
  type CodeUiClient,
  type CodeUiEventStream,
  type CodeUiTransport,
} from "./client";
import { BrowserControllerProvider, useBrowserController, type BrowserController } from "./controller";
import { commandFixture, executionFixture, interactionFixture, repairFixture, sessionFixture, sseFixture } from "./fixtures";
import { isTerminalPhase, phaseForSnapshot } from "./phases";
import { CodeUiStoreProvider, useCodeUiStore } from "./store";
import type {
  CodeUiControllerAttachResponse,
  CodeUiEventEnvelope,
  CodeUiSessionSnapshot,
} from "./types";
import { toShellViewModel } from "./view-model";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

class ControllableCodeUiClient implements CodeUiClient {
  readonly calls: Array<{ name: string; args: unknown[] }> = [];
  readonly observers = new Set<{
    onEvent: (event: CodeUiEventEnvelope) => void;
    onError: () => void;
  }>();
  currentSnapshot = sessionFixture();

  snapshot = vi.fn(async () => this.currentSnapshot);
  attach = vi.fn(async (): Promise<CodeUiControllerAttachResponse> => ({
    controllerToken: "controller-token",
    leaseExpiresAt: new Date(Date.now() + 60_000).toISOString(),
    controller: this.currentSnapshot.controller,
  }));
  detach = vi.fn(async (clientId: string, token: string, keepalive = false): Promise<void> => {
    this.calls.push({ name: "detach", args: [clientId, token, keepalive] });
  });
  submit = vi.fn<CodeUiClient["submit"]>(async () => ({
    accepted: true,
  }));
  respond = vi.fn<CodeUiClient["respond"]>(async () => ({ accepted: true }));
  cancel = vi.fn<CodeUiClient["cancel"]>(async () => ({ accepted: true }));

  observe = (
    onEvent: (event: CodeUiEventEnvelope) => void,
    onError: () => void,
  ): CodeUiEventStream => {
    const observer = { onEvent, onError };
    this.observers.add(observer);
    return { close: () => this.observers.delete(observer) };
  };

  emit(event: CodeUiEventEnvelope): void {
    this.currentSnapshot = event.data;
    this.observers.forEach(({ onEvent }) => onEvent(event));
  }

  fail(): void {
    this.observers.forEach(({ onError }) => onError());
  }
}

function StoreProbe({ onStore }: { onStore: (store: ReturnType<typeof useCodeUiStore>) => void }) {
  onStore(useCodeUiStore());
  return null;
}

function ControllerProbe({
  onController,
}: {
  onController: (controller: BrowserController) => void;
}) {
  onController(useBrowserController());
  return null;
}

async function mount(element: ReturnType<typeof createElement>): Promise<{ root: Root; container: HTMLDivElement }> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(element);
  });
  return { root, container };
}

async function unmount(root: Root, container: HTMLDivElement): Promise<void> {
  await act(async () => {
    root.unmount();
  });
  container.remove();
}

describe("Code UI shared foundation", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    document.body.replaceChildren();
  });

  it("expresses composable interaction, execution, repair, and SSE fixtures", () => {
    const snapshot = executionFixture({
      interactions: [interactionFixture()],
      planExecutionRepair: repairFixture(),
      toolCalls: [commandFixture({ status: "failed" })],
    });
    const event = sseFixture(snapshot, { type: "future_domain_event" });

    expect(event.type).toBe("future_domain_event");
    expect(event.data.planExecutionRepair?.state).toBe("awaiting_user");
    expect(event.data.interactions).toHaveLength(1);
  });

  it("dispatches snapshots through the mock observer", async () => {
    const client = new MockCodeUiClient(sessionFixture());
    const seen: number[] = [];
    const stream = client.observe((event) => seen.push(event.seq), () => undefined);
    await act(async () => {
      client.emit(sseFixture(sessionFixture({ status: "thinking" }), { seq: 2 }));
    });
    await client.submit("hello", "token", "command-1");
    stream.close();

    expect(seen).toEqual([2]);
    expect(client.calls).toContainEqual({ name: "submit", args: ["hello", "token", "command-1"] });
  });

  it("rejects oversized messages before sending them", async () => {
    let requestCount = 0;
    const transport: CodeUiTransport = {
      request: async <T,>() => {
        requestCount += 1;
        return { accepted: true } as T;
      },
      events: () => ({ close: () => undefined }),
    };
    const client = createCodeUiClient(transport);

    await expect(client.submit("x".repeat(256 * 1024), "token")).rejects.toMatchObject({
      code: "PAYLOAD_TOO_LARGE",
      status: 413,
    });

    expect(requestCount).toBe(0);
  });

  it("reuses a sessionStorage browser client id across remounts", async () => {
    window.sessionStorage.clear();
    const client = new ControllableCodeUiClient();
    const seen: string[] = [];
    const first = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          null,
          createElement(ControllerProbe, {
            onController: (value) => {
              seen.push(value.clientId);
            },
          }),
        ),
      ),
    );
    await unmount(first.root, first.container);

    const second = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          null,
          createElement(ControllerProbe, {
            onController: (value) => {
              seen.push(value.clientId);
            },
          }),
        ),
      ),
    );
    await unmount(second.root, second.container);

    expect(seen).toHaveLength(2);
    expect(seen[0]).toBeTruthy();
    expect(seen[1]).toBe(seen[0]);
    expect(window.sessionStorage.getItem("libra.code-ui.browser-client-id")).toBe(seen[0]);
  });

  it("parses nested API errors so an invalid controller token can be retried", async () => {
    const client = new ControllableCodeUiClient();
    const apiClient = createCodeUiClient();
    let requestCount = 0;
    const fetchMock = vi.fn(async () => {
      requestCount += 1;
      if (requestCount === 1) {
        return new Response(JSON.stringify({
          error: { code: "INVALID_CONTROLLER_TOKEN", message: "Controller lease expired" },
        }), { status: 401, statusText: "Unauthorized" });
      }
      return new Response(JSON.stringify({ accepted: true }), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    client.submit.mockImplementation((text, token, commandId) => apiClient.submit(text, token, commandId));
    let controller: BrowserController | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "browser-test" },
          createElement(ControllerProbe, { onController: (value) => (controller = value) }),
        ),
      ),
    );

    await controller?.submit("retry me");

    expect(client.attach).toHaveBeenCalledTimes(2);
    expect(client.submit).toHaveBeenCalledTimes(2);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    await unmount(root, container);
  });

  it("derives a presentation-neutral shell model", () => {
    const snapshot = sessionFixture({
      status: "awaiting_interaction",
      interactions: [interactionFixture()],
    });
    expect(phaseForSnapshot(snapshot)).toBe("waiting");
    expect(isTerminalPhase("reconcile")).toBe(true);
    expect(toShellViewModel(snapshot)).toMatchObject({
      phase: "waiting",
      pendingInteractionCount: 1,
      canWrite: false,
    });
  });

  it("loads the initial snapshot and subscribes to SSE updates", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(store?.snapshot?.status).toBe("idle");
    expect(client.observers.size).toBe(1);

    await act(async () => {
      client.emit(sseFixture(sessionFixture({ status: "thinking" }), { seq: 2 }));
    });
    expect(store?.snapshot?.status).toBe("thinking");
    await unmount(root, container);
  });

  it("ignores a fetched snapshot superseded by an SSE event", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    let resolveSnapshot: ((snapshot: CodeUiSessionSnapshot) => void) | undefined;
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((resolve) => {
          resolveSnapshot = resolve;
        }),
    );
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      client.emit(sseFixture(sessionFixture({ status: "thinking" }), { seq: 2 }));
    });
    await act(async () => {
      resolveSnapshot?.(sessionFixture({ status: "idle" }));
    });

    expect(store?.snapshot?.status).toBe("thinking");
    await unmount(root, container);
  });

  it("does not let an older overlapping refresh replace a newer result", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    const resolveSnapshots: Array<(snapshot: CodeUiSessionSnapshot) => void> = [];
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((resolve) => {
          resolveSnapshots.push(resolve);
        }),
    );
    const olderRefresh = store?.refresh();
    const newerRefresh = store?.refresh();
    let newerResult: Awaited<ReturnType<NonNullable<typeof store>["refresh"]>> | undefined;
    let olderResult: Awaited<ReturnType<NonNullable<typeof store>["refresh"]>> | undefined;
    await act(async () => {
      resolveSnapshots[1]?.(sessionFixture({ status: "thinking" }));
      newerResult = await newerRefresh;
    });
    await act(async () => {
      resolveSnapshots[0]?.(sessionFixture({ status: "idle" }));
      olderResult = await olderRefresh;
    });

    expect(newerResult).toBe("applied");
    expect(olderResult).toBe("raced");
    expect(store?.snapshot?.status).toBe("thinking");
    await unmount(root, container);
  });

  it("does not let an older refresh failure clobber a newer refresh before any snapshot exists", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    const settle: Array<{
      resolve: (snapshot: CodeUiSessionSnapshot) => void;
      reject: (cause: Error) => void;
    }> = [];
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((resolve, reject) => {
          settle.push({ resolve, reject });
        }),
    );
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client, reconnectDelayMs: 60_000 },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(settle).toHaveLength(1);

    const newerRefresh = store!.refresh();
    expect(settle).toHaveLength(2);

    let newerResult: Awaited<ReturnType<NonNullable<typeof store>["refresh"]>> | undefined;
    // Reject the older (bootstrap) refresh after the newer one has started.
    await act(async () => {
      settle[0]!.reject(new Error("stale bootstrap failure"));
      await Promise.resolve();
    });

    await act(async () => {
      settle[1]!.resolve(sessionFixture({ status: "thinking" }));
      newerResult = await newerRefresh;
    });

    expect(newerResult).toBe("applied");
    expect(store?.error).toBeUndefined();
    expect(store?.snapshot?.status).toBe("thinking");
    await unmount(root, container);
  });

  it("returns superseded when a live SSE update owns newer data than the refresh", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(store?.snapshot?.status).toBe("idle");

    let resolveSnapshot: ((snapshot: CodeUiSessionSnapshot) => void) | undefined;
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((resolve) => {
          resolveSnapshot = resolve;
        }),
    );
    const pending = store?.refresh();
    let result: Awaited<ReturnType<NonNullable<typeof store>["refresh"]>> | undefined;
    await act(async () => {
      client.emit(
        sseFixture(sessionFixture({ status: "executing_tool", updatedAt: "2026-07-15T00:00:09.000Z" }), {
          seq: 3,
        }),
      );
      resolveSnapshot?.(
        sessionFixture({ status: "idle", updatedAt: "2026-07-15T00:00:01.000Z" }),
      );
      result = await pending;
    });

    expect(result).toBe("superseded");
    expect(store?.snapshot?.status).toBe("executing_tool");
    await unmount(root, container);
  });

  it("keeps a newer refresh that resolves after a stale initial SSE snapshot", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    let resolveSnapshot: ((snapshot: CodeUiSessionSnapshot) => void) | undefined;
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((resolve) => {
          resolveSnapshot = resolve;
        }),
    );
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      client.emit(
        sseFixture(sessionFixture({ status: "thinking", updatedAt: "2026-07-15T00:00:00.000Z" }), {
          seq: 2,
        }),
      );
    });
    await act(async () => {
      resolveSnapshot?.(
        sessionFixture({
          status: "awaiting_interaction",
          interactions: [interactionFixture()],
          updatedAt: "2026-07-15T00:00:01.000Z",
        }),
      );
    });

    expect(store?.snapshot).toMatchObject({
      status: "awaiting_interaction",
      interactions: [{ id: "interaction-fixture", status: "pending" }],
    });
    await unmount(root, container);
  });

  it("keeps a fractional-second refresh newer than a whole-second SSE snapshot", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    let resolveSnapshot: ((snapshot: CodeUiSessionSnapshot) => void) | undefined;
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((resolve) => {
          resolveSnapshot = resolve;
        }),
    );
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      client.emit(
        sseFixture(sessionFixture({ status: "thinking", updatedAt: "2026-07-15T00:00:00Z" }), {
          seq: 2,
        }),
      );
    });
    await act(async () => {
      resolveSnapshot?.(
        sessionFixture({
          status: "awaiting_interaction",
          updatedAt: "2026-07-15T00:00:00.001Z",
        }),
      );
    });

    expect(store?.snapshot?.status).toBe("awaiting_interaction");
    await unmount(root, container);
  });

  it("accepts a fractional-second SSE update after a whole-second refresh", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    client.currentSnapshot = sessionFixture({
      status: "thinking",
      updatedAt: "2026-07-15T00:00:00Z",
    });
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
      client.emit(
        sseFixture(
          sessionFixture({
            status: "awaiting_interaction",
            updatedAt: "2026-07-15T00:00:00.001Z",
          }),
          { seq: 2 },
        ),
      );
    });

    expect(store?.snapshot?.status).toBe("awaiting_interaction");
    await unmount(root, container);
  });

  it("does not replace a newer fetched snapshot with a stale SSE snapshot", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    client.currentSnapshot = sessionFixture({ updatedAt: "2026-07-15T00:00:00.000Z" });
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    client.currentSnapshot = sessionFixture({
      status: "awaiting_interaction",
      updatedAt: "2026-07-15T00:00:02.000Z",
    });
    await act(async () => {
      await store?.refresh();
      client.emit(sseFixture(sessionFixture({
        status: "thinking",
        updatedAt: "2026-07-15T00:00:01.000Z",
      }), { seq: 2 }));
    });

    expect(store?.snapshot?.status).toBe("awaiting_interaction");
    await unmount(root, container);
  });

  it("clears a fetch error when a valid SSE snapshot arrives", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    client.snapshot.mockRejectedValueOnce(new Error("temporary session failure"));
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(store?.error?.message).toBe("temporary session failure");
    await act(async () => {
      client.emit(sseFixture(sessionFixture({ status: "thinking" }), { seq: 2 }));
    });

    expect(store?.snapshot?.status).toBe("thinking");
    expect(store?.error).toBeUndefined();
    await unmount(root, container);
  });

  it("does not show a superseded refresh failure after an SSE snapshot", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    let rejectSnapshot: ((cause: Error) => void) | undefined;
    client.snapshot.mockImplementation(
      () =>
        new Promise<CodeUiSessionSnapshot>((_resolve, reject) => {
          rejectSnapshot = reject;
        }),
    );
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(StoreProbe, { onStore: (value) => (store = value) }),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
      client.emit(sseFixture(sessionFixture({ status: "thinking" }), { seq: 2 }));
    });
    await act(async () => {
      rejectSnapshot?.(new Error("temporary session failure"));
    });

    expect(store?.snapshot?.status).toBe("thinking");
    expect(store?.error).toBeUndefined();
    await unmount(root, container);
  });

  it("backs off reconnect attempts and resets after a live SSE event", async () => {
    vi.useFakeTimers();
    const client = new ControllableCodeUiClient();
    const { root, container } = await mount(createElement(CodeUiStoreProvider, { client }));

    await act(async () => {
      client.fail();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(999);
    });
    expect(client.observers.size).toBe(0);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.observers.size).toBe(1);

    await act(async () => {
      client.fail();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_999);
    });
    expect(client.observers.size).toBe(0);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.observers.size).toBe(1);

    await act(async () => {
      client.emit(sseFixture(sessionFixture({ status: "thinking" }), { seq: 3 }));
      client.fail();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(client.observers.size).toBe(1);
    await unmount(root, container);
  });

  it("detaches an acquired browser controller lease on unmount", async () => {
    const client = new ControllableCodeUiClient();
    let controller: BrowserController | undefined;
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "browser-test" },
          createElement(ControllerProbe, { onController: (value) => (controller = value) }),
        ),
      ),
    );

    await controller?.ensureLease();
    await unmount(root, container);
    expect(client.detach).toHaveBeenCalledWith("browser-test", "controller-token", true);
  });
});
