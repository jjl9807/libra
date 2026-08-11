// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { BrowserControllerProvider } from "../../../lib/code-ui/controller";
import {
  busySessionFixture,
  createSessionLifecycleApi,
  terminalSessionFixture,
  threadListFixture,
} from "../../../lib/code-ui/session-lifecycle";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";

import { ResumeCancelPanel } from "./ResumeCancelPanel";
import { SessionLifecycle } from "./SessionLifecycle";
import { SessionLifecycleHost } from "./SessionLifecycleHost";
import { ThreadListPanel } from "./ThreadListPanel";

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

describe("workspace session-lifecycle", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("selects a thread from the list panel", async () => {
    const onSelect = vi.fn();
    const { root, container } = await mount(
      createElement(ThreadListPanel, {
        items: threadListFixture().items,
        onRefresh: vi.fn(),
        onLoadMore: vi.fn(),
        onSelect,
      }),
    );

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent?.includes("Earlier turn"),
      ) as HTMLButtonElement).click();
    });
    expect(onSelect).toHaveBeenCalledWith("thread-fixture-2");
    await unmount(root, container);
  });

  it("surfaces cancel failure and resume readiness in the host", async () => {
    const onCancel = vi.fn();
    const onResume = vi.fn();
    const { root, container } = await mount(
      createElement(SessionLifecycleHost, {
        items: threadListFixture().items,
        selectedThreadId: "thread-fixture-2",
        currentThreadId: "thread-fixture",
        phaseLabel: "Finished",
        affordance: {
          kind: "ready",
          reason: "Browser resume HTTP lands in W3-01.",
        },
        cancelError: "lease expired",
        onRefreshThreads: vi.fn(),
        onLoadMoreThreads: vi.fn(),
        onSelectThread: vi.fn(),
        onCancelTurn: onCancel,
        onResumeIntent: onResume,
      }),
    );

    expect(container.textContent).toContain("lease expired");
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Cancel turn",
      ) as HTMLButtonElement).click();
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Prepare resume",
      ) as HTMLButtonElement).click();
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onResume).toHaveBeenCalledTimes(1);
    await unmount(root, container);
  });

  it("defers resume while the session is busy", async () => {
    const { root, container } = await mount(
      createElement(ResumeCancelPanel, {
        currentThreadId: "thread-fixture",
        selectedThreadId: "thread-fixture-2",
        phaseLabel: "Thinking",
        affordance: {
          kind: "deferred",
          reason: "Wait for the active turn to settle before resuming another thread.",
        },
        onCancel: vi.fn(),
        onResumeIntent: vi.fn(),
      }),
    );

    const resume = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Prepare resume",
    ) as HTMLButtonElement;
    expect(resume.disabled).toBe(true);
    expect(container.textContent).toContain("Wait for the active turn");
    await unmount(root, container);
  });

  it("cancels through SessionLifecycle and explains process-level resume", async () => {
    const client = new MockCodeUiClient(
      terminalSessionFixture({
        threadId: "thread-fixture",
        controller: { kind: "browser", canWrite: true, loopbackOnly: true },
      }),
    );
    const cancel = vi.fn(async () => ({ accepted: true }));
    client.cancel = cancel;
    const api = createSessionLifecycleApi({
      async request<T>(): Promise<T> {
        return threadListFixture() as T;
      },
    });

    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "session-lifecycle-test" },
          createElement(SessionLifecycle, { api }),
        ),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(container.textContent).toContain("Prior investigation");
    expect(container.textContent).toContain("Phase: Finished");

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent?.includes("Earlier turn"),
      ) as HTMLButtonElement).click();
    });

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Prepare resume",
      ) as HTMLButtonElement).click();
    });
    expect(container.textContent).toMatch(/libra code --resume thread-fixture-2/);
    expect(container.textContent).toMatch(/working directory/i);

    const cancelButton = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Cancel turn",
    ) as HTMLButtonElement;
    expect(cancelButton.disabled).toBe(false);

    await act(async () => {
      cancelButton.click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(cancel).toHaveBeenCalledTimes(1);
    await unmount(root, container);
  });

  it("fail-closes cancel when the controller cannot write", async () => {
    const client = new MockCodeUiClient(
      busySessionFixture({
        threadId: "thread-fixture",
        controller: { kind: "none", canWrite: false, loopbackOnly: true },
        capabilities: {
          ...busySessionFixture().capabilities,
          providerSessionResume: false,
        },
      }),
    );
    const cancelMock = vi.fn(async () => ({ accepted: true }));
    client.cancel = cancelMock;
    const api = createSessionLifecycleApi({
      async request<T>(): Promise<T> {
        return threadListFixture() as T;
      },
    });

    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "session-lifecycle-test" },
          createElement(SessionLifecycle, { api }),
        ),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const cancelButton = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Cancel turn",
    ) as HTMLButtonElement;
    expect(cancelButton.disabled).toBe(true);
    expect(cancelMock).not.toHaveBeenCalled();
    await unmount(root, container);
  });

  it("surfaces cancel failure feedback through SessionLifecycle", async () => {
    const client = new MockCodeUiClient(
      terminalSessionFixture({
        threadId: "thread-fixture",
        controller: { kind: "browser", canWrite: true, loopbackOnly: true },
      }),
    );
    client.cancel = vi.fn(async () => {
      throw { status: 409, code: "CONTROLLER_CONFLICT", message: "lease held elsewhere" };
    });
    const api = createSessionLifecycleApi({
      async request<T>(): Promise<T> {
        return threadListFixture() as T;
      },
    });

    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "session-lifecycle-cancel-fail" },
          createElement(SessionLifecycle, { api }),
        ),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Cancel turn",
      ) as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(container.textContent).toContain("lease held elsewhere");
    await unmount(root, container);
  });
});
