"use client";

import type { GoalStatusView } from "../../../lib/code-ui/goal-task-skill";
import type { SkillActivation, DiscoveredSkill } from "../../../lib/code-ui/goal-task-skill";
import type { CodeUiTaskSnapshot } from "../../../lib/code-ui/types";

import { GoalPanel } from "./GoalPanel";
import { SkillSearchPanel } from "./SkillSearchPanel";
import { TaskPanel } from "./TaskPanel";

export interface GoalTaskSkillHostProps {
  goalStatus?: GoalStatusView;
  tasks: CodeUiTaskSnapshot[];
  skills?: DiscoveredSkill[];
  busy?: boolean;
  goalError?: string;
  taskError?: string;
  skillError?: string;
  lastSkillActivation?: string;
  lastTaskResult?: string;
  onStartGoal(objective: string): void | Promise<void>;
  onRefreshGoal(): void | Promise<void>;
  onCancelGoal(reason: string): void | Promise<void>;
  onDispatchTask(agent: string, prompt: string): void | Promise<void>;
  onActivateSkill(activation: SkillActivation): void | Promise<void>;
}

export function GoalTaskSkillHost({
  goalStatus,
  tasks,
  skills,
  busy,
  goalError,
  taskError,
  skillError,
  lastSkillActivation,
  lastTaskResult,
  onStartGoal,
  onRefreshGoal,
  onCancelGoal,
  onDispatchTask,
  onActivateSkill,
}: GoalTaskSkillHostProps) {
  return (
    <div aria-label="Goal task skill workspace">
      <GoalPanel
        status={goalStatus}
        busy={busy}
        error={goalError}
        onStart={onStartGoal}
        onRefresh={onRefreshGoal}
        onCancel={onCancelGoal}
      />
      <TaskPanel
        tasks={tasks}
        busy={busy}
        error={taskError}
        lastResult={lastTaskResult}
        onDispatch={onDispatchTask}
      />
      <SkillSearchPanel
        skills={skills}
        busy={busy}
        error={skillError}
        lastActivation={lastSkillActivation}
        onActivate={onActivateSkill}
      />
    </div>
  );
}

export { GoalPanel } from "./GoalPanel";
export { TaskPanel } from "./TaskPanel";
export { SkillSearchPanel } from "./SkillSearchPanel";
