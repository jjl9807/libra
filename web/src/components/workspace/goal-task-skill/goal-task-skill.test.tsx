// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  discoveredSkillsFixture,
  goalStatusFixture,
  tasksSessionFixture,
} from "../../../lib/code-ui/goal-task-skill";
import { MockCodeUiClient } from "../../../lib/code-ui/client";
import { BrowserControllerProvider } from "../../../lib/code-ui/controller";
import { CodeUiStoreProvider } from "../../../lib/code-ui/store";
import { GoalPanel } from "./GoalPanel";
import { GoalTaskSkillHost } from "./GoalTaskSkillHost";
import { SessionGoalTaskSkill } from "./SessionGoalTaskSkill";
import { SkillSearchPanel } from "./SkillSearchPanel";
import { TaskPanel } from "./TaskPanel";
import { createGoalTaskSkillApi } from "../../../lib/code-ui/goal-task-skill";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Flush CodeUiStoreProvider's setTimeout(0) bootstrap + snapshot promise. */
async function flushStoreBootstrap(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  await act(async () => {
    await Promise.resolve();
  });
}

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

describe("workspace goal-task-skill", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("starts, refreshes, and cancels a goal", async () => {
    const onStart = vi.fn();
    const onRefresh = vi.fn();
    const onCancel = vi.fn();
    const { root, container } = await mount(
      createElement(GoalPanel, {
        status: goalStatusFixture(),
        onStart,
        onRefresh,
        onCancel,
      }),
    );

    const objective = container.querySelector('textarea[aria-label="Goal objective"]') as HTMLTextAreaElement;
    await act(async () => {
      setFieldValue(objective, "Ship panels");
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Start goal") as HTMLButtonElement).click();
    });
    expect(onStart).toHaveBeenCalledWith("Ship panels");

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Refresh status") as HTMLButtonElement).click();
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Cancel goal") as HTMLButtonElement).click();
    });
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledWith("cancelled from browser");
    await unmount(root, container);
  });

  it("dispatches a task from the task panel", async () => {
    const onDispatch = vi.fn();
    const { root, container } = await mount(
      createElement(TaskPanel, {
        tasks: tasksSessionFixture().tasks,
        onDispatch,
      }),
    );

    expect(container.textContent).toContain("Draft release notes");
    const agent = container.querySelector('input[aria-label="Task agent"]') as HTMLInputElement;
    const prompt = container.querySelector('textarea[aria-label="Task prompt"]') as HTMLTextAreaElement;
    await act(async () => {
      setFieldValue(agent, "explorer");
      setFieldValue(prompt, "inspect failures");
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Dispatch task") as HTMLButtonElement).click();
    });
    expect(onDispatch).toHaveBeenCalledWith("explorer", "inspect failures");
    await unmount(root, container);
  });

  it("searches and activates an A0-07 skill", async () => {
    const onActivate = vi.fn();
    const { root, container } = await mount(
      createElement(SkillSearchPanel, {
        skills: discoveredSkillsFixture(),
        onActivate,
      }),
    );

    const provider = container.querySelector('input[aria-label="Skill provider filter"]') as HTMLInputElement;
    await act(async () => {
      setFieldValue(provider, "codex");
    });
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) =>
        button.textContent?.includes("Validate /review (codex)"),
      ) as HTMLButtonElement).click();
    });
    expect(onActivate).toHaveBeenCalledWith({ provider: "codex", name: "/review" });
    await unmount(root, container);
  });

  it("hosts goal/task/skill panels together under the store providers", async () => {
    const onStartGoal = vi.fn();
    const onRefreshGoal = vi.fn();
    const onCancelGoal = vi.fn();
    const onDispatchTask = vi.fn();
    const onActivateSkill = vi.fn();
    const client = new MockCodeUiClient(tasksSessionFixture());
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          null,
          createElement(GoalTaskSkillHost, {
            goalStatus: goalStatusFixture(),
            tasks: tasksSessionFixture().tasks,
            onStartGoal,
            onRefreshGoal,
            onCancelGoal,
            onDispatchTask,
            onActivateSkill,
          }),
        ),
      ),
    );

    expect(container.querySelector('[aria-label="Goal task skill workspace"]')).not.toBeNull();
    expect(container.textContent).toContain("Goal goal-fixture");
    expect(container.textContent).toContain("Investigate failing tests");
    expect(container.textContent).toContain("A0-07 curated discovery");
    await unmount(root, container);
  });

  it("starts a goal through SessionGoalTaskSkill with a leased controller token", async () => {
    const tokens: string[] = [];
    const cancelCalls: Array<{ method?: string; reason?: string }> = [];
    let activeGoal: string | undefined;
    const api = createGoalTaskSkillApi({
      async request<T>(path: string, init?: RequestInit): Promise<T> {
        if (path === "/api/code/goal/status") {
          if (!activeGoal) {
            throw {
              status: 422,
              code: "UNSUPPORTED_OPERATION",
              message: "no active Goal in this session — start one with goal.start",
            };
          }
          return { status: activeGoal } as T;
        }
        if (path === "/api/code/goal/start") {
          const headerBag = new Headers(init?.headers);
          tokens.push(headerBag.get("X-Code-Controller-Token") ?? "");
          activeGoal = "Goal live-1 — Running\nObjective: leased write";
          return { accepted: true, status: activeGoal } as T;
        }
        if (path === "/api/code/goal/cancel") {
          const headerBag = new Headers(init?.headers);
          tokens.push(headerBag.get("X-Code-Controller-Token") ?? "");
          const body = JSON.parse(String(init?.body ?? "{}")) as { reason?: string };
          cancelCalls.push({ method: init?.method, reason: body.reason });
          activeGoal = undefined;
          return {
            accepted: true,
            status: "Goal live-1 — Cancelled\nObjective: leased write",
          } as T;
        }
        if (path === "/api/code/task/dispatch") {
          const headerBag = new Headers(init?.headers);
          tokens.push(headerBag.get("X-Code-Controller-Token") ?? "");
          return { accepted: true, result: "Task helper completed" } as T;
        }
        throw new Error(`unexpected ${path}`);
      },
    });
    const client = new MockCodeUiClient(tasksSessionFixture());
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "goal-task-skill-test" },
          createElement(SessionGoalTaskSkill, { api }),
        ),
      ),
    );

    await flushStoreBootstrap();
    expect(container.textContent).toContain("No active Goal status loaded");

    const objective = container.querySelector('textarea[aria-label="Goal objective"]') as HTMLTextAreaElement;
    await act(async () => {
      setFieldValue(objective, "leased write");
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Start goal") as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(client.calls.some((call) => call.name === "attach")).toBe(true);
    expect(tokens[0]).toBe("mock-controller-token");
    expect(container.textContent).toContain("Goal live-1");
    expect(container.textContent).toContain("Objective: leased write");

    activeGoal = "Goal live-1 — Running\nObjective: refreshed objective";
    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Refresh status") as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(container.textContent).toContain("refreshed objective");

    await act(async () => {
      const agent = container.querySelector('input[aria-label="Task agent"]') as HTMLInputElement;
      const prompt = container.querySelector('textarea[aria-label="Task prompt"]') as HTMLTextAreaElement;
      setFieldValue(agent, "explorer");
      setFieldValue(prompt, "run helper");
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Dispatch task") as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(tokens.at(-1)).toBe("mock-controller-token");
    expect(container.textContent).toContain("Task helper completed");

    await act(async () => {
      (Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "Cancel goal") as HTMLButtonElement).click();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(cancelCalls).toEqual([{ method: "POST", reason: "cancelled from browser" }]);
    expect(tokens.at(-1)).toBe("mock-controller-token");
    expect(container.textContent).toContain("Cancelled");
    await unmount(root, container);
  });

  it("probes Goal status once on mount rather than on every snapshot tick", async () => {
    let statusCalls = 0;
    const api = createGoalTaskSkillApi({
      async request<T>(path: string): Promise<T> {
        if (path === "/api/code/goal/status") {
          statusCalls += 1;
          throw {
            status: 409,
            code: "GOAL_NOT_ACTIVE",
            message: "No Goal is active in this session — call goal.start first",
          };
        }
        throw new Error(`unexpected ${path}`);
      },
    });
    const client = new MockCodeUiClient(tasksSessionFixture());
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "goal-status-once" },
          createElement(SessionGoalTaskSkill, { api }),
        ),
      ),
    );
    await flushStoreBootstrap();
    expect(statusCalls).toBe(1);
    expect(container.textContent).toContain("No active Goal status loaded");
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(statusCalls).toBe(1);
    await unmount(root, container);
  });

  it("hides Goal/task UI for managed Codex and skips goal status probe", async () => {
    let statusCalls = 0;
    const api = createGoalTaskSkillApi({
      async request<T>(): Promise<T> {
        statusCalls += 1;
        throw new Error("managed Codex must not probe goal status");
      },
    });
    const client = new MockCodeUiClient(
      tasksSessionFixture({
        provider: { provider: "codex", managed: true },
      }),
    );
    const { root, container } = await mount(
      createElement(
        CodeUiStoreProvider,
        { client },
        createElement(
          BrowserControllerProvider,
          { clientId: "codex-goal-hidden" },
          createElement(SessionGoalTaskSkill, { api }),
        ),
      ),
    );
    await flushStoreBootstrap();
    expect(container.querySelector('[aria-label="Goal task skill workspace"]')).toBeNull();
    expect(statusCalls).toBe(0);
    await unmount(root, container);
  });
});
