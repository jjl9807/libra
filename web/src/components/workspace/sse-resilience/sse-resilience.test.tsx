// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { sessionFixture, sseFixture } from "../../../lib/code-ui/fixtures";
import {
  reconnectingSseState,
  resyncRequiredSseState,
  resyncedSseState,
  wrapClientForSseResilience,
} from "../../../lib/code-ui/sse-resilience";
import { CodeUiStoreProvider, useCodeUiStore } from "../../../lib/code-ui/store";

import { SessionSseResilience } from "./SessionSseResilience";
import { SseResilienceHost } from "./SseResilienceHost";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function StoreProbe({
  onStore,
}: {
  onStore: (value: ReturnType<typeof useCodeUiStore>) => void;
}) {
  onStore(useCodeUiStore());
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

describe("workspace sse-resilience", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("shows reconnecting without implying the snapshot was cleared", async () => {
    const { root, container } = await mount(
      createElement(SseResilienceHost, {
        state: reconnectingSseState(),
        onResync: vi.fn(),
      }),
    );
    expect(container.textContent).toContain("SSE reconnecting");
    expect(container.textContent).toContain("Last cursor seq: 4");
    expect(container.textContent).toContain("Projected session snapshot is retained");
    await unmount(root, container);
  });

  it("surfaces backlog overflow and resynced states", async () => {
    const onResync = vi.fn();
    const { root, container } = await mount(
      createElement(SseResilienceHost, {
        state: resyncRequiredSseState(),
        onResync,
      }),
    );
    expect(container.textContent).toContain("SSE resync required");
    expect(container.textContent).toContain("SSE backlog exceeded");
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Resync snapshot",
      ) as HTMLButtonElement).click();
    });
    expect(onResync).toHaveBeenCalledTimes(1);
    await unmount(root, container);

    const resynced = await mount(
      createElement(SseResilienceHost, {
        state: resyncedSseState(),
        onResync: vi.fn(),
      }),
    );
    expect(resynced.container.textContent).toContain("SSE resynced");
    await unmount(resynced.root, resynced.container);
  });

  it("tracks wire seq, surfaces disconnect, and fails closed on bad resync", async () => {
    const base = new MockCodeUiClient(sessionFixture());
    const disconnectors = new Set<() => void>();
    const nativeObserve = base.observe.bind(base);
    base.observe = (onEvent, onError) => {
      disconnectors.add(onError);
      return nativeObserve(onEvent, onError);
    };
    const client = wrapClientForSseResilience(base);
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(SessionSseResilience),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    await act(async () => {
      base.emit(sseFixture(sessionFixture({ updatedAt: "2026-07-15T00:00:01.000Z" }), { seq: 3 }));
    });
    expect(container.textContent).toContain("Last cursor seq: 3");

    await act(async () => {
      disconnectors.forEach((disconnect) => disconnect());
    });
    expect(container.textContent).toContain("SSE reconnecting");
    expect(container.textContent).toContain("Last cursor seq: 3");
    expect(container.textContent).toContain("Projected session snapshot is retained");

    await act(async () => {
      base.emit(sseFixture(sessionFixture({ updatedAt: "2026-07-15T00:00:02.000Z" }), { seq: 0 }));
    });
    expect(container.textContent).toContain("Last cursor seq: 3");

    await unmount(root, container);

    const failing = new MockCodeUiClient(sessionFixture());
    failing.snapshot = vi.fn(async () => {
      throw new Error("session unavailable");
    });
    const second = await mount(
      createElement(
        CodeUiStoreProvider,
        { client: wrapClientForSseResilience(failing) },
        createElement(SessionSseResilience, { initialState: resyncRequiredSseState() }),
      ),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    await act(async () => {
      (Array.from(second.container.querySelectorAll("button")).find((button) =>
        button.textContent === "Resync snapshot",
      ) as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(failing.snapshot).toHaveBeenCalled();
    expect(second.container.textContent).toContain("SSE resync required");
    expect(second.container.textContent).not.toContain("SSE resynced");
    expect(second.container.textContent).toContain("Unable to refresh Code UI session snapshot");
    await unmount(second.root, second.container);

    const okClient = new MockCodeUiClient(
      sessionFixture({ updatedAt: "2026-07-15T00:00:01.000Z" }),
    );
    okClient.snapshot = vi.fn(async () =>
      sessionFixture({ updatedAt: "2026-07-15T00:00:03.000Z", status: "completed" }),
    );
    let store: ReturnType<typeof useCodeUiStore> | undefined;
    const third = await mount(
      createElement(
        CodeUiStoreProvider,
        { client: wrapClientForSseResilience(okClient) },
        createElement(
          "div",
          null,
          createElement(StoreProbe, { onStore: (value) => (store = value) }),
          createElement(SessionSseResilience, { initialState: resyncRequiredSseState() }),
        ),
      ),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    await act(async () => {
      (Array.from(third.container.querySelectorAll("button")).find((button) =>
        button.textContent === "Resync snapshot",
      ) as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(okClient.snapshot).toHaveBeenCalled();
    expect(store?.snapshot?.updatedAt).toBe("2026-07-15T00:00:03.000Z");
    expect(store?.snapshot?.status).toBe("completed");
    expect(third.container.textContent).toContain("SSE resynced");
    expect(third.container.textContent).toContain("Projected session snapshot is retained");
    await unmount(third.root, third.container);
  });
});
