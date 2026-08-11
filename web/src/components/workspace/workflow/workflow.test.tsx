// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  intentReviewSessionFixture,
  networkPolicySessionFixture,
  planReviewSessionFixture,
  workflowView,
} from "../../../lib/code-ui/workflow";

import { WorkflowHost } from "./WorkflowHost";

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

describe("workspace workflow", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("posts IntentSpec confirm/modify/cancel selections", async () => {
    const onRespond = vi.fn();
    const { root, container } = await mount(
      createElement(WorkflowHost, {
        view: workflowView(intentReviewSessionFixture()),
        onRespond,
        onCancelTurn: vi.fn(),
      }),
    );
    expect(container.textContent).toContain("IntentSpec review");
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Confirm",
      ) as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledWith("intent-review-1", {
      selectedOption: "confirm",
      answers: {},
    });
    await unmount(root, container);
  });

  it("posts plan execute and keeps network policy as an explicit gate", async () => {
    const onRespond = vi.fn();
    const plan = await mount(
      createElement(WorkflowHost, {
        view: workflowView(planReviewSessionFixture()),
        onRespond,
        onCancelTurn: vi.fn(),
      }),
    );
    expect(plan.container.textContent).toContain("Plan review");
    expect(plan.container.textContent).toContain("Inspect repository");
    await act(async () => {
      (Array.from(plan.container.querySelectorAll("button")).find((button) =>
        button.textContent === "Execute",
      ) as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledWith("plan-review-1", {
      selectedOption: "execute",
      answers: {},
    });
    await unmount(plan.root, plan.container);

    const network = await mount(
      createElement(WorkflowHost, {
        view: workflowView(networkPolicySessionFixture()),
        onRespond,
        onCancelTurn: vi.fn(),
      }),
    );
    expect(network.container.textContent).toContain("Network policy");
    expect(network.container.textContent).toContain("explicit choice required");
    await act(async () => {
      (Array.from(network.container.querySelectorAll("button")).find((button) =>
        button.textContent === "Allow network",
      ) as HTMLButtonElement).click();
    });
    expect(onRespond).toHaveBeenCalledWith("plan-1:network-policy", {
      selectedOption: "network-allow",
      answers: {},
    });
    await unmount(network.root, network.container);
  });

  it("surfaces validation errors for unsupported options and fail-closes cancel", async () => {
    const onCancelTurn = vi.fn();
    const invalid = workflowView(intentReviewSessionFixture());
    invalid.interaction = {
      ...invalid.interaction!,
      options: [{ id: "nope", label: "Nope" }],
    };
    const { root, container } = await mount(
      createElement(WorkflowHost, {
        view: invalid,
        onRespond: vi.fn(),
        onCancelTurn,
        cancelEnabled: false,
      }),
    );
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent === "Nope",
      ) as HTMLButtonElement).click();
    });
    expect(container.textContent).toContain("Unsupported workflow option");
    const cancel = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Cancel turn",
    ) as HTMLButtonElement;
    expect(cancel.disabled).toBe(true);
    await unmount(root, container);
  });
});
