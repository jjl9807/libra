import { describe, expect, it } from "vitest";

import {
  A0_07_DISCOVERED_SKILLS,
  activateDiscoveredSkill,
  createGoalTaskSkillApi,
  discoveredSkillsFixture,
  goalStatusFixture,
  isAbsentGoalError,
  parseGoalStatus,
  searchDiscoveredSkills,
  sortTasks,
  taskFixture,
  tasksSessionFixture,
  validateObjective,
  validateSkillActivation,
  validateTaskDispatch,
} from ".";

describe("goal-task-skill helpers", () => {
  it("parses goal status text and validates start objective", () => {
    const view = parseGoalStatus(goalStatusFixture().raw);
    expect(view.id).toBe("goal-fixture");
    expect(view.state).toBe("Running");
    expect(view.objective).toBe("Ship W2-09 goal/task/skill panels");
    expect(validateObjective("")).toMatch(/objective/i);
    expect(validateObjective("  ship it  ")).toBeUndefined();
    expect(
      isAbsentGoalError({
        status: 422,
        code: "UNSUPPORTED_OPERATION",
        message: "no active Goal in this session — start one with goal.start",
      }),
    ).toBe(true);
    expect(
      isAbsentGoalError({
        status: 409,
        code: "GOAL_NOT_ACTIVE",
        message: "No Goal is active in this session — call goal.start first",
      }),
    ).toBe(true);
    expect(isAbsentGoalError({ message: "controller conflict" })).toBe(false);
    expect(validateObjective("a".repeat(16 * 1024 + 1))).toMatch(/16 KiB/i);
  });

  it("sorts snapshot tasks newest-first and validates dispatch", () => {
    const tasks = sortTasks(tasksSessionFixture().tasks);
    expect(tasks.map((task) => task.id)).toEqual(["task-fixture-2", "task-fixture"]);
    expect(validateTaskDispatch("", "prompt")).toMatch(/agent/i);
    expect(validateTaskDispatch("helper", "")).toMatch(/prompt/i);
    expect(validateTaskDispatch("helper", "inspect")).toBeUndefined();
    expect(taskFixture({ status: "done" }).status).toBe("done");
  });

  it("searches and activates only A0-07 curated skills", () => {
    expect(discoveredSkillsFixture()).toEqual([...A0_07_DISCOVERED_SKILLS]);
    expect(searchDiscoveredSkills({ provider: "claude-code" }).map((skill) => skill.name)).toEqual([
      "/review",
      "/security-review",
      "/simplify",
    ]);
    expect(searchDiscoveredSkills({ skill: "/review", provider: "codex" })).toEqual([
      { name: "/review", provider: "codex" },
    ]);
    expect(validateSkillActivation({ provider: "gemini", name: "/review" })).toMatch(/not discoverable/);
    expect(activateDiscoveredSkill({ provider: "opencode", name: "/review" })).toEqual({
      accepted: true,
      message:
        "Discoverable: /review for opencode (runtime activation awaits Code UI skill HTTP)",
    });
  });

  it("posts goal start/status/cancel and task dispatch through the domain API", async () => {
    const transport = {
      async request<T>(path: string): Promise<T> {
        if (path === "/api/code/goal/start") {
          return { accepted: true, status: "Goal g1 — Running\nObjective: x" } as T;
        }
        if (path === "/api/code/goal/status") {
          return { status: "Goal g1 — Running\nObjective: x" } as T;
        }
        if (path === "/api/code/goal/cancel") {
          return { accepted: true, status: "Goal g1 — Cancelled" } as T;
        }
        if (path === "/api/code/task/dispatch") {
          return { accepted: true, result: "dispatched" } as T;
        }
        throw new Error(`unexpected ${path}`);
      },
    };
    const api = createGoalTaskSkillApi(transport);
    await expect(api.startGoal("x", "lease")).resolves.toEqual({
      accepted: true,
      status: "Goal g1 — Running\nObjective: x",
    });
    await expect(api.goalStatus()).resolves.toEqual({
      status: "Goal g1 — Running\nObjective: x",
    });
    await expect(api.cancelGoal("done", "lease")).resolves.toEqual({
      accepted: true,
      status: "Goal g1 — Cancelled",
    });
    await expect(api.dispatchTask("helper", "inspect", "lease")).resolves.toEqual({
      accepted: true,
      result: "dispatched",
    });
  });
});
