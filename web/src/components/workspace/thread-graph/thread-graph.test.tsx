// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { sessionFixture, sseFixture } from "../../../lib/code-ui/fixtures";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";
import {
  INDEXED_THREAD_GRAPH_LOAD_FAILED,
  INDEXED_THREAD_GRAPH_UNAVAILABLE,
  threadGraphView,
} from "../../../lib/code-ui/thread-graph";

import { SessionThreadGraph } from "./SessionThreadGraph";
import { ThreadGraphHost } from "./ThreadGraphHost";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

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

describe("workspace thread graph", () => {
  afterEach(() => {
    document.body.replaceChildren();
    vi.unstubAllGlobals();
  });

  it("lists intent/plan/task nodes from the snapshot graph", async () => {
    const snapshot = sessionFixture({
      threadId: "thread-1",
      threadGraph: {
        threadId: "thread-1",
        title: "Review thread",
        nodes: [
          { depth: 0, kind: "intent", id: "intent-1", label: "Intent 1", tags: ["current"] },
          { depth: 1, kind: "plan", id: "plan-1", label: "Plan 1", tags: ["selected"] },
        ],
      },
    });
    const { root, container } = await mount(
      createElement(ThreadGraphHost, { view: threadGraphView(snapshot) }),
    );
    expect(container.textContent).toContain("Thread version graph");
    expect(container.textContent).toContain("Intent 1");
    expect(container.textContent).toContain("Plan 1");
    expect(container.textContent).toContain("current");
    await unmount(root, container);
  });

  it("mounts from the live store snapshot", async () => {
    const client = new MockCodeUiClient(
      sessionFixture({
        threadId: "thread-live",
        threadGraph: {
          threadId: "thread-live",
          nodes: [{ depth: 2, kind: "task", id: "task-1", label: "Active task", tags: ["active"] }],
        },
      }),
    );
    const { root, container } = await mount(
      createElement(CodeUiStoreProvider, { client }, createElement(SessionThreadGraph)),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toContain("Active task");
    await unmount(root, container);
  });

  it("marks live fallback as partial after the indexed graph is dropped", async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        JSON.stringify({
          error: { code: "THREAD_GRAPH_NOT_FOUND", message: "no thread projection" },
        }),
        { status: 404, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    const client = new MockCodeUiClient(
      sessionFixture({
        threadId: "thread-live",
        threadGraph: {
          threadId: "thread-live",
          nodes: [{ depth: 1, kind: "plan", id: "plan-1", label: "Plan 1", tags: ["selected"] }],
        },
      }),
    );
    const { root, container } = await mount(
      createElement(CodeUiStoreProvider, { client }, createElement(SessionThreadGraph)),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toContain("Plan 1");

    await act(async () => {
      client.emit(
        sseFixture(
          sessionFixture({
            threadId: "thread-live",
            plans: [
              {
                id: "plan-2",
                title: "New plan",
                status: "selected",
                steps: [],
                updatedAt: "2026-07-15T00:00:00.000Z",
              },
            ],
          }),
          { seq: 2 },
        ),
      );
    });
    expect(container.textContent).toContain(INDEXED_THREAD_GRAPH_UNAVAILABLE);
    expect(container.textContent).toContain("New plan");
    await unmount(root, container);
  });

  it("surfaces indexed graph loader failures instead of a missing-graph fallback", async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        JSON.stringify({
          error: { code: "THREAD_GRAPH_UNAVAILABLE", message: "projection storage failed" },
        }),
        { status: 500, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    try {
      const client = new MockCodeUiClient(
        sessionFixture({
          threadId: "thread-live",
          plans: [
            {
              id: "plan-1",
              title: "Live plan",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
        }),
      );
      const { root, container } = await mount(
        createElement(CodeUiStoreProvider, { client }, createElement(SessionThreadGraph)),
      );
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
      });
      expect(fetchMock).toHaveBeenCalled();
      expect(container.textContent).toContain(INDEXED_THREAD_GRAPH_LOAD_FAILED);
      expect(container.textContent).toContain("THREAD_GRAPH_UNAVAILABLE");
      expect(container.textContent).toContain("Live plan");
      expect(container.textContent).not.toContain(INDEXED_THREAD_GRAPH_UNAVAILABLE);
      await unmount(root, container);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("keeps a fetched indexed graph after a snapshot revision without lineage changes", async () => {
    let resolveFetch: ((response: Response) => void) | undefined;
    const fetchMock = vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          resolveFetch = resolve;
        }),
    );
    vi.stubGlobal("fetch", fetchMock);
    try {
      const livePlan = {
        id: "plan-1",
        title: "Live plan",
        status: "selected" as const,
        steps: [],
        updatedAt: "2026-07-15T00:00:00.000Z",
      };
      const client = new MockCodeUiClient(
        sessionFixture({
          threadId: "thread-live",
          updatedAt: "2026-07-15T00:00:00.000Z",
          plans: [livePlan],
        }),
      );
      const { root, container } = await mount(
        createElement(CodeUiStoreProvider, { client }, createElement(SessionThreadGraph)),
      );
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
      expect(fetchMock).toHaveBeenCalledTimes(1);

      await act(async () => {
        client.emit(
          sseFixture(
            sessionFixture({
              threadId: "thread-live",
              status: "thinking",
              updatedAt: "2026-07-15T00:00:01.000Z",
              plans: [livePlan],
            }),
            { seq: 2, at: "2026-07-15T00:00:01.000Z" },
          ),
        );
      });

      await act(async () => {
        resolveFetch?.(
          new Response(
            JSON.stringify({
              threadId: "thread-live",
              title: "Indexed thread",
              nodes: [
                { depth: 0, kind: "intent", id: "intent-1", label: "Indexed intent", tags: ["current"] },
                { depth: 1, kind: "plan", id: "plan-1", label: "Indexed plan", tags: ["selected"] },
              ],
            }),
            { status: 200, headers: { "content-type": "application/json" } },
          ),
        );
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
      expect(container.textContent).toContain("Indexed intent");
      expect(container.textContent).not.toContain(INDEXED_THREAD_GRAPH_UNAVAILABLE);

      await act(async () => {
        client.emit(
          sseFixture(
            sessionFixture({
              threadId: "thread-live",
              status: "executing_tool",
              updatedAt: "2026-07-15T00:00:02.000Z",
              plans: [livePlan],
            }),
            { seq: 3, at: "2026-07-15T00:00:02.000Z" },
          ),
        );
      });
      expect(container.textContent).toContain("Indexed intent");
      expect(container.textContent).not.toContain(INDEXED_THREAD_GRAPH_UNAVAILABLE);

      await act(async () => {
        client.emit(
          sseFixture(
            sessionFixture({
              threadId: "thread-live",
              status: "executing_tool",
              updatedAt: "2026-07-15T00:00:02.000Z",
              plans: [{ ...livePlan, status: "completed", updatedAt: "2026-07-15T00:00:02.000Z" }],
            }),
            { seq: 4, at: "2026-07-15T00:00:02.000Z" },
          ),
        );
      });
      expect(fetchMock.mock.calls.length).toBeGreaterThan(1);
      await unmount(root, container);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("drops a fetched graph when live heads are replaced without changing list length", async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        JSON.stringify({
          threadId: "thread-live",
          title: "Indexed thread",
          nodes: [
            { depth: 0, kind: "intent", id: "intent-1", label: "Indexed intent", tags: ["current"] },
            { depth: 1, kind: "plan", id: "plan-1", label: "Indexed plan", tags: ["selected"] },
          ],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    try {
      const client = new MockCodeUiClient(
        sessionFixture({
          threadId: "thread-live",
          plans: [
            {
              id: "plan-1",
              title: "Live plan",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
        }),
      );
      const { root, container } = await mount(
        createElement(CodeUiStoreProvider, { client }, createElement(SessionThreadGraph)),
      );
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
      });
      expect(container.textContent).toContain("Indexed intent");

      await act(async () => {
        client.emit(
          sseFixture(
            sessionFixture({
              threadId: "thread-live",
              updatedAt: "2026-07-15T00:00:02.000Z",
              plans: [
                {
                  id: "plan-2",
                  title: "Replacement plan",
                  status: "selected",
                  steps: [],
                  updatedAt: "2026-07-15T00:00:02.000Z",
                },
              ],
            }),
            { seq: 2, at: "2026-07-15T00:00:02.000Z" },
          ),
        );
      });
      expect(container.textContent).toContain("Replacement plan");
      expect(container.textContent).toContain(INDEXED_THREAD_GRAPH_UNAVAILABLE);
      expect(container.textContent).not.toContain("Indexed intent");
      await unmount(root, container);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("treats THREAD_GRAPH_NOT_FOUND as a missing indexed graph", async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        JSON.stringify({
          error: { code: "THREAD_GRAPH_NOT_FOUND", message: "no thread projection" },
        }),
        { status: 404, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    try {
      const client = new MockCodeUiClient(
        sessionFixture({
          threadId: "thread-live",
          plans: [
            {
              id: "plan-1",
              title: "Live plan",
              status: "selected",
              steps: [],
              updatedAt: "2026-07-15T00:00:00.000Z",
            },
          ],
        }),
      );
      const { root, container } = await mount(
        createElement(CodeUiStoreProvider, { client }, createElement(SessionThreadGraph)),
      );
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
      });
      expect(fetchMock).toHaveBeenCalled();
      expect(container.textContent).toContain(INDEXED_THREAD_GRAPH_UNAVAILABLE);
      expect(container.textContent).not.toContain(INDEXED_THREAD_GRAPH_LOAD_FAILED);
      expect(container.textContent).toContain("Live plan");
      await unmount(root, container);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
