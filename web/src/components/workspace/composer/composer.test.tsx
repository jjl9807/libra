// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { BrowserControllerProvider } from "../../../lib/code-ui/controller";
import { sessionFixture } from "../../../lib/code-ui/fixtures";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";

import { SessionComposer } from "./SessionComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function setFieldValue(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const prototype =
    element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
  setter?.call(element, value);
  element.dispatchEvent(new Event("input", { bubbles: true }));
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

describe("workspace composer", () => {
  afterEach(() => {
    vi.useRealTimers();
    document.body.replaceChildren();
  });

  it("submits trimmed text through the browser controller", async () => {
    vi.useFakeTimers();
    const snapshot = sessionFixture({
      capabilities: {
        ...sessionFixture().capabilities,
        messageInput: true,
      },
      controller: { kind: "browser", canWrite: false, loopbackOnly: true },
    });
    const client = new MockCodeUiClient(snapshot);
    const submitSpy = vi.spyOn(client, "submit");
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "composer-test" },
          createElement(SessionComposer),
        ),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    const textarea = container.querySelector('textarea[aria-label="Session message"]') as HTMLTextAreaElement;
    expect(textarea).toBeTruthy();
    await act(async () => {
      setFieldValue(textarea, "  hello  ");
    });
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Send",
      ) as HTMLButtonElement).click();
    });

    expect(submitSpy).toHaveBeenCalledWith(
      "hello",
      "mock-controller-token",
      expect.any(String),
    );
    await unmount(root, container);
  });

  it("hides the form when messageInput capability is off", async () => {
    vi.useFakeTimers();
    const snapshot = sessionFixture({
      capabilities: {
        ...sessionFixture().capabilities,
        messageInput: false,
      },
    });
    const client = new MockCodeUiClient(snapshot);
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "composer-disabled-test" },
          createElement(SessionComposer),
        ),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    expect(container.textContent).toContain("Message input is not available for this session.");
    expect(container.querySelector("textarea")).toBeNull();
    await unmount(root, container);
  });
});
