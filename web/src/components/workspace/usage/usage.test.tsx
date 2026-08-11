// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { sessionFixture } from "../../../lib/code-ui/fixtures";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";
import {
  createUsageApi,
  totalsFixture,
  usageReadModelFixture,
} from "../../../lib/code-ui/usage";

import { SessionUsage } from "./SessionUsage";
import { UsageHost } from "./UsageHost";

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

describe("workspace usage", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("shows cumulative, turn delta, and sub-agent totals", async () => {
    const { root, container } = await mount(
      createElement(UsageHost, {
        model: usageReadModelFixture(),
        onRefresh: vi.fn(),
      }),
    );

    expect(container.textContent).toContain("Cumulative");
    expect(container.textContent).toContain("Current turn");
    expect(container.textContent).toContain("Sub-agent: reviewer");
    expect(container.textContent).toMatch(/partial/);
    expect(container.textContent).toMatch(/unknown_usage=/);
    expect(container.textContent).toContain("Incomplete usage");
    await unmount(root, container);
  });

  it("keeps empty state explicit instead of fake zeros", async () => {
    const { root, container } = await mount(
      createElement(UsageHost, {
        deferredHint: "Usage query is unavailable for this session.",
        onRefresh: vi.fn(),
      }),
    );
    expect(container.textContent).toContain("No usage totals loaded.");
    expect(container.textContent).toContain("No current-turn delta loaded.");
    expect(container.textContent).toContain("Sub-agent attribution is unavailable.");
    expect(container.textContent).not.toMatch(/Requests\s*0/);
    await unmount(root, container);
  });

  it("refreshes through SessionUsage without inflating on identical payloads", async () => {
    let calls = 0;
    const paths: string[] = [];
    const payload = usageReadModelFixture();
    const api = createUsageApi({
      async request<T>(path: string): Promise<T> {
        calls += 1;
        paths.push(path);
        return payload as T;
      },
    });
    const client = new MockCodeUiClient(
      sessionFixture({ threadId: "thread-fixture" }),
    );
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(SessionUsage, { api }),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toContain("Sub-agent: reviewer");
    expect(calls).toBe(1);
    expect(paths[0]).toBe(
      "/api/code/usage?sessionId=session-fixture&threadId=thread-fixture",
    );
    expect(paths[0]).not.toContain("turnId=");

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Refresh usage",
      ) as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(calls).toBe(2);
    expect(container.textContent).toContain("Requests");
    expect(container.textContent).toContain("3");
    await unmount(root, container);
  });

  it("surfaces fetch errors and deferred 404 without fabricating known zeros", async () => {
    const api = createUsageApi({
      async request<T>(): Promise<T> {
        throw { status: 404, code: "CODE_UI_UNAVAILABLE", message: "missing" };
      },
    });
    const client = new MockCodeUiClient(sessionFixture());
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(SessionUsage, { api }),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toMatch(/unavailable for this session/i);
    expect(container.textContent).toContain("No usage totals loaded.");
    expect(container.textContent).toContain("Sub-agent attribution is unavailable.");
    await unmount(root, container);
  });

  it("drops stale usage responses when session scope changes", async () => {
    let resolveFirst: ((value: ReturnType<typeof usageReadModelFixture>) => void) | undefined;
    const first = new Promise<ReturnType<typeof usageReadModelFixture>>((resolve) => {
      resolveFirst = resolve;
    });
    let calls = 0;
    const api = createUsageApi({
      async request<T>(path: string): Promise<T> {
        calls += 1;
        if (calls === 1) {
          expect(path).toContain("sessionId=session-a");
          return (await first) as T;
        }
        expect(path).toContain("sessionId=session-b");
        return usageReadModelFixture({
          sessionId: "session-b",
          cumulative: totalsFixture({ requestCount: 2, totalTokens: 20 }),
        }) as T;
      },
    });
    const client = new MockCodeUiClient(
      sessionFixture({ sessionId: "session-a", threadId: "thread-a" }),
    );
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(SessionUsage, { api }),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(calls).toBe(1);

    await act(async () => {
      client.emit({
        seq: 2,
        type: "session_updated",
        at: "2026-07-15T00:00:01.000Z",
        data: sessionFixture({
          sessionId: "session-b",
          threadId: "thread-b",
          updatedAt: "2026-07-15T00:00:01.000Z",
        }),
      });
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(calls).toBe(2);

    await act(async () => {
      resolveFirst?.(
        usageReadModelFixture({
          sessionId: "session-a",
          cumulative: totalsFixture({ requestCount: 99, totalTokens: 999 }),
        }),
      );
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(container.textContent).toContain("20");
    expect(container.textContent).not.toContain("999");
    await unmount(root, container);
  });

  it("shows injected initial model without requiring HTTP", async () => {
    const client = new MockCodeUiClient(sessionFixture());
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(SessionUsage, {
          initialModel: usageReadModelFixture({
            cumulative: totalsFixture({ requestCount: 7, totalTokens: 42 }),
          }),
          api: createUsageApi({
            async request<T>(): Promise<T> {
              throw new Error("should not fetch when initialModel is set on mount");
            },
          }),
        }),
      ),
    );

    expect(container.textContent).toContain("42");
    expect(container.textContent).toContain("7");
    await unmount(root, container);
  });
});
