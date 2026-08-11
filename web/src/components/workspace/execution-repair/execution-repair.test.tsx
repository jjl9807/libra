// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { BrowserControllerProvider } from "../../../lib/code-ui/controller";
import {
  awaitingRepairSessionFixture,
  automaticRepairSessionFixture,
  executionRepairView,
  exhaustedRepairSessionFixture,
  manualActionRepairFixture,
} from "../../../lib/code-ui/execution-repair";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";

import { ExecutionRepairHost } from "./ExecutionRepairHost";
import { RepairPanel } from "./RepairPanel";
import { SessionExecutionRepair } from "./SessionExecutionRepair";

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

describe("workspace execution-repair", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("shows execution progress and repair evidence from the host", async () => {
    const onContinue = vi.fn();
    const onCancel = vi.fn();
    const { root, container } = await mount(
      createElement(ExecutionRepairHost, {
        view: executionRepairView(awaitingRepairSessionFixture()),
        canRespond: true,
        onContinue,
        onCancelRepair: onCancel,
      }),
    );

    expect(container.textContent).toContain("Ship fix");
    expect(container.textContent).toContain("Run tests — failed");
    expect(container.textContent).toContain("verification failed");
    expect(container.textContent).toContain("test failure");
    expect(container.textContent).toContain("Attempt 1 / 2");

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Continue repair",
      ) as HTMLButtonElement).click();
    });
    expect(onContinue).toHaveBeenCalledWith(
      "repair-interaction",
      expect.objectContaining({ selectedOption: "continue", answers: {} }),
    );
    await unmount(root, container);
  });

  it("raises maxAttempts on Continue when the projected budget is exhausted", async () => {
    const onContinue = vi.fn();
    const { root, container } = await mount(
      createElement(RepairPanel, {
        repair: exhaustedRepairSessionFixture().planExecutionRepair,
        interactionId: "repair-interaction",
        canRespond: true,
        onContinue,
        onCancel: vi.fn(),
      }),
    );

    expect(container.textContent).toContain("raise maxAttempts to 3");
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Continue repair",
      ) as HTMLButtonElement).click();
    });
    expect(onContinue).toHaveBeenCalledWith(
      "repair-interaction",
      expect.objectContaining({ selectedOption: "continue", maxAttempts: 3 }),
    );
    await unmount(root, container);
  });

  it("surfaces manual action without inventing continue controls when not awaiting", async () => {
    const { root, container } = await mount(
      createElement(RepairPanel, {
        repair: manualActionRepairFixture(),
        onContinue: vi.fn(),
        onCancel: vi.fn(),
      }),
    );
    expect(container.textContent).toContain("Manual action");
    expect(container.querySelectorAll("button")).toHaveLength(0);
    await unmount(root, container);
  });

  it("responds through SessionExecutionRepair with a leased controller", async () => {
    const client = new MockCodeUiClient(awaitingRepairSessionFixture());
    client.respond = vi.fn(async () => ({ accepted: true }));
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "execution-repair-test" },
          createElement(SessionExecutionRepair),
        ),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Cancel repair",
      ) as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(client.respond).toHaveBeenCalledWith(
      "repair-interaction",
      expect.objectContaining({ selectedOption: "cancel" }),
      "mock-controller-token",
    );
    await unmount(root, container);
  });

  it("shows repair actions when no controller lease exists yet", async () => {
    const client = new MockCodeUiClient(
      awaitingRepairSessionFixture({
        controller: { kind: "none", canWrite: false, loopbackOnly: true },
      }),
    );
    client.respond = vi.fn(async () => ({ accepted: true }));
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "execution-repair-no-lease" },
          createElement(SessionExecutionRepair),
        ),
      ),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const continueButton = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Continue repair",
    ) as HTMLButtonElement;
    expect(continueButton).toBeTruthy();
    expect(continueButton.disabled).toBe(false);

    await act(async () => {
      continueButton.click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(client.calls.some((call) => call.name === "attach")).toBe(true);
    expect(client.respond).toHaveBeenCalled();
    await unmount(root, container);
  });

  it("hides respond controls during automatic repair", async () => {
    const { root, container } = await mount(
      createElement(ExecutionRepairHost, {
        view: executionRepairView(automaticRepairSessionFixture()),
        canRespond: true,
        onContinue: vi.fn(),
        onCancelRepair: vi.fn(),
      }),
    );
    expect(container.textContent).toContain("Automatic repair");
    expect(
      Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Continue repair",
      ),
    ).toBeUndefined();
    await unmount(root, container);
  });
});
