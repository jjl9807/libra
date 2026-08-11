import { sessionFixture } from "../fixtures";
import type { CodeUiSessionSnapshot, CodeUiTaskSnapshot } from "../types";

import type { GoalStatusView } from "./goal";
import { A0_07_DISCOVERED_SKILLS, type DiscoveredSkill } from "./skill";

const now = "2026-07-15T00:00:00.000Z";

export function taskFixture(
  overrides: Partial<CodeUiTaskSnapshot> = {},
): CodeUiTaskSnapshot {
  return {
    id: "task-fixture",
    title: "Investigate failing tests",
    status: "active",
    details: "Active scheduler task",
    updatedAt: now,
    ...overrides,
  };
}

export function tasksSessionFixture(
  overrides: Partial<CodeUiSessionSnapshot> = {},
): CodeUiSessionSnapshot {
  return sessionFixture({
    tasks: [
      taskFixture(),
      taskFixture({
        id: "task-fixture-2",
        title: "Draft release notes",
        status: "queued",
        details: "Waiting for approval",
        updatedAt: "2026-07-15T00:01:00.000Z",
      }),
    ],
    ...overrides,
  });
}

export function goalStatusFixture(overrides: Partial<GoalStatusView> = {}): GoalStatusView {
  return {
    raw: [
      "Goal goal-fixture — Running",
      "Objective: Ship W2-09 goal/task/skill panels",
      "Acceptance criteria:",
      "- panels mount from runtime projection",
    ].join("\n"),
    id: "goal-fixture",
    state: "Running",
    objective: "Ship W2-09 goal/task/skill panels",
    ...overrides,
  };
}

export function discoveredSkillsFixture(
  overrides: DiscoveredSkill[] = [...A0_07_DISCOVERED_SKILLS],
): DiscoveredSkill[] {
  return overrides.map((skill) => ({ ...skill }));
}
