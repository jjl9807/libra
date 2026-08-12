// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  sandboxApprovalFixture,
  toolApprovalFixture,
  userInputFixture,
} from "../../../lib/code-ui/interactions";
import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { BrowserControllerProvider } from "../../../lib/code-ui/controller";
import { sessionFixture } from "../../../lib/code-ui/fixtures";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";
import { ApprovalPanel } from "./ApprovalPanel";
import { InteractionsHost } from "./InteractionsHost";
import { SessionInteractions } from "./SessionInteractions";

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

describe("workspace interactions", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("resolves and cancels a tool approval", async () => {
    const onRespond = vi.fn();
    const onCancel = vi.fn();
    const { root, container } = await mount(createElement(ApprovalPanel, {
      interaction: toolApprovalFixture(),
      onRespond,
      onCancel,
    }));

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Approve") as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledWith("tool-approval-fixture", {
      approved: true,
      selectedOption: "approve",
      applyToFuture: "no",
      answers: {},
    });
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Cancel") as HTMLButtonElement).click();
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    await unmount(root, container);
  });

  it("resolves and cancels a sandbox approval", async () => {
    const onRespond = vi.fn();
    const onCancel = vi.fn();
    const { root, container } = await mount(createElement(ApprovalPanel, {
      interaction: sandboxApprovalFixture(),
      onRespond,
      onCancel,
    }));

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Deny") as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledWith("sandbox-approval-fixture", expect.objectContaining({
      approved: false,
      selectedOption: "deny",
    }));
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Cancel") as HTMLButtonElement).click();
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    await unmount(root, container);
  });

  it.each([
    ["accept_all", "approve"],
    ["decline_all", "deny"],
    ["no", "approve"],
  ] as const)("round-trips %s apply-to-future choice", async (applyToFuture, optionLabel) => {
    const onRespond = vi.fn();
    const { root, container } = await mount(createElement(ApprovalPanel, {
      interaction: toolApprovalFixture(),
      onRespond,
      onCancel: vi.fn(),
    }));
    const select = container.querySelector("select") as HTMLSelectElement;
    await act(async () => {
      select.value = applyToFuture;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === optionLabel[0].toUpperCase() + optionLabel.slice(1)) as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledWith("tool-approval-fixture", expect.objectContaining({ applyToFuture }));
    await unmount(root, container);
  });

  it("resets approval apply-to-future when the interaction id changes", async () => {
    const onRespond = vi.fn();
    const first = toolApprovalFixture({ id: "approval-one" });
    const second = toolApprovalFixture({ id: "approval-two" });
    const { root, container } = await mount(createElement(InteractionsHost, {
      interaction: first,
      onRespond,
      onCancel: vi.fn(),
    }));

    const select = container.querySelector("select") as HTMLSelectElement;
    await act(async () => {
      select.value = "accept_all";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(select.value).toBe("accept_all");

    await act(async () => {
      root.render(createElement(InteractionsHost, {
        interaction: second,
        onRespond,
        onCancel: vi.fn(),
      }));
    });
    expect((container.querySelector("select") as HTMLSelectElement).value).toBe("no");
    await unmount(root, container);
  });

  it("bridges SessionInteractions through store + leased controller", async () => {
    vi.useFakeTimers();
    const client = new MockCodeUiClient(
      sessionFixture({
        status: "awaiting_interaction",
        controller: { kind: "browser", canWrite: true, loopbackOnly: true },
        interactions: [toolApprovalFixture()],
      }),
    );
    const respondSpy = vi.spyOn(client, "respond");
    const attachSpy = vi.spyOn(client, "attach");

    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "browser-bridge-test" },
          createElement(SessionInteractions),
        ),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(container.querySelector('[aria-label="Approval request"]')).toBeTruthy();

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Approve") as HTMLButtonElement).click();
    });
    expect(attachSpy).toHaveBeenCalled();
    expect(respondSpy).toHaveBeenCalledWith(
      "tool-approval-fixture",
      expect.objectContaining({ selectedOption: "approve", applyToFuture: "no" }),
      client.controllerToken,
    );
    await unmount(root, container);
    vi.useRealTimers();
  });

  it("surfaces a failed approval response so the user can retry", async () => {
    const onRespond = vi.fn(async () => {
      throw { message: "Controller lease expired", status: 401 };
    });
    const { root, container } = await mount(createElement(ApprovalPanel, {
      interaction: toolApprovalFixture(),
      onRespond,
      onCancel: vi.fn(),
    }));

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Approve") as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledTimes(1);
    expect(container.querySelector("[role=alert]")?.textContent).toMatch(/Controller lease expired/);
    expect(
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Approve") as HTMLButtonElement)
        .disabled,
    ).toBe(false);
    await unmount(root, container);
  });

  it("submits complete answers and blocks invalid input before responding", async () => {
    const onRespond = vi.fn();
    const onCancel = vi.fn();
    const { root, container } = await mount(createElement(InteractionsHost, {
      interaction: userInputFixture(),
      onRespond,
      onCancel,
    }));
    const form = container.querySelector("form") as HTMLFormElement;
    await act(async () => {
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    expect(container.querySelector("[role=alert]")?.textContent).toMatch(/risk profile/i);
    expect(onRespond).not.toHaveBeenCalled();

    const [select, textarea, secret] = Array.from(
      container.querySelectorAll("select, textarea, input[type=password]"),
    ) as [HTMLSelectElement, HTMLTextAreaElement, HTMLInputElement];
    expect(secret).toBeTruthy();
    expect(Array.from(select.options).some((option) => option.text === "None of the above")).toBe(true);
    await act(async () => {
      select.value = "low";
      select.dispatchEvent(new Event("change", { bubbles: true }));
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(
        textarea,
        "Use the local rollout.",
      );
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(secret, "tok");
      secret.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    expect(onRespond).toHaveBeenCalledWith("user-input-fixture", {
      answers: {
        risk_profile: ["low"],
        additional_context: ["Use the local rollout."],
        deploy_token: ["tok"],
      },
    });
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Cancel") as HTMLButtonElement).click();
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    await unmount(root, container);
  });

  it("enables managed Codex approval respond through SessionInteractions (W3-07)", async () => {
    vi.useFakeTimers();
    const client = new MockCodeUiClient(
      sessionFixture({
        status: "awaiting_interaction",
        provider: { provider: "codex", managed: true },
        controller: { kind: "browser", canWrite: true, loopbackOnly: true },
        interactions: [toolApprovalFixture()],
      }),
    );
    const respondSpy = vi.spyOn(client, "respond");

    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "codex-bridge-test" },
          createElement(SessionInteractions),
        ),
      ),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const approve = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Approve",
    ) as HTMLButtonElement;
    expect(approve.disabled).toBe(false);
    expect(container.querySelector("[role=status]")).toBeNull();
    await act(async () => {
      approve.click();
    });
    expect(respondSpy).toHaveBeenCalledTimes(1);
    expect(respondSpy.mock.calls[0]?.[0]).toBe(toolApprovalFixture().id);
    await unmount(root, container);
    vi.useRealTimers();
  });
});
