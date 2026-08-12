"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useBrowserController } from "@/lib/code-ui/controller";
import {
  createGoalTaskSkillApi,
  isAbsentGoalError,
  parseGoalStatus,
  validateDiscoveredSkill,
  validateObjective,
  validateSkillActivation,
  validateTaskDispatch,
  type GoalStatusView,
  type GoalTaskSkillApi,
  type SkillActivation,
} from "@/lib/code-ui/goal-task-skill";
import { useCodeUiStore } from "@/lib/code-ui/store";

import { GoalTaskSkillHost } from "./GoalTaskSkillHost";

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Request failed. Try again.";
}

export interface SessionGoalTaskSkillProps {
  /** Test seam — production leaves this unset and uses same-origin fetch. */
  api?: GoalTaskSkillApi;
}

/**
 * Wires goal/task HTTP + A0-07 skill discovery to the browser controller lease.
 * Lives in the W2-09 ownership tree so the foundation page only mounts it.
 */
export function SessionGoalTaskSkill({ api: injectedApi }: SessionGoalTaskSkillProps) {
  const { snapshot } = useCodeUiStore();
  const controller = useBrowserController();
  const api = useMemo(() => injectedApi ?? createGoalTaskSkillApi(), [injectedApi]);
  const [goalStatus, setGoalStatus] = useState<GoalStatusView>();
  const [busy, setBusy] = useState(false);
  const [goalError, setGoalError] = useState<string>();
  const [taskError, setTaskError] = useState<string>();
  const [skillError, setSkillError] = useState<string>();
  const [lastSkillActivation, setLastSkillActivation] = useState<string>();
  const [lastTaskResult, setLastTaskResult] = useState<string>();
  const refreshGeneration = useRef(0);
  const busyRef = useRef(false);

  // Managed Codex adapters do not implement Goal/task HTTP (default
  // unsupported stubs). Hide the panel instead of probing and surfacing errors.
  const goalTaskSupported = !(
    snapshot?.provider.managed === true &&
    snapshot.provider.provider.toLowerCase() === "codex"
  );

  const run = useCallback(async (operation: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await operation();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  const refreshGoal = useCallback(async (options?: { allowWhileBusy?: boolean }) => {
    // `run(refreshGoal)` sets busyRef first; allow that path to proceed.
    if (busyRef.current && !options?.allowWhileBusy) return;
    const generation = ++refreshGeneration.current;
    setGoalError(undefined);
    try {
      const response = await api.goalStatus();
      if (generation !== refreshGeneration.current) return;
      setGoalStatus(parseGoalStatus(response.status));
    } catch (cause) {
      if (generation !== refreshGeneration.current) return;
      if (isAbsentGoalError(cause)) {
        setGoalStatus(undefined);
        return;
      }
      setGoalError(errorMessage(cause));
    }
  }, [api]);

  useEffect(() => {
    if (!snapshot || !goalTaskSupported) return;
    // Probe once per session id; do not re-fetch on every SSE tick.
    void refreshGoal();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- session-scoped Goal probe
  }, [snapshot?.sessionId, goalTaskSupported]);

  // Wait for the session snapshot so managed Codex can be gated before any
  // Goal/task HTTP probe or controls appear.
  if (!snapshot || !goalTaskSupported) {
    return null;
  }

  return (
    <GoalTaskSkillHost
      goalStatus={goalStatus}
      tasks={snapshot?.tasks ?? []}
      busy={busy}
      goalError={goalError}
      taskError={taskError}
      skillError={skillError}
      lastSkillActivation={lastSkillActivation}
      lastTaskResult={lastTaskResult}
      onStartGoal={(objective) =>
        void run(async () => {
          setGoalError(undefined);
          const validation = validateObjective(objective);
          if (validation) {
            setGoalError(validation);
            return;
          }
          // Invalidate in-flight status reads before the mutating write.
          refreshGeneration.current += 1;
          try {
            const response = await controller.withLease((token) =>
              api.startGoal(objective.trim(), token),
            );
            refreshGeneration.current += 1;
            setGoalStatus(parseGoalStatus(response.status));
          } catch (cause) {
            setGoalError(errorMessage(cause));
          }
        })
      }
      onRefreshGoal={() => void run(async () => refreshGoal({ allowWhileBusy: true }))}
      onCancelGoal={(reason) =>
        void run(async () => {
          setGoalError(undefined);
          refreshGeneration.current += 1;
          try {
            const response = await controller.withLease((token) =>
              api.cancelGoal(reason.trim() || "cancelled from browser", token),
            );
            refreshGeneration.current += 1;
            setGoalStatus(parseGoalStatus(response.status));
          } catch (cause) {
            if (isAbsentGoalError(cause)) {
              refreshGeneration.current += 1;
              setGoalStatus(undefined);
              return;
            }
            setGoalError(errorMessage(cause));
          }
        })
      }
      onDispatchTask={(agent, prompt) =>
        void run(async () => {
          setTaskError(undefined);
          const validation = validateTaskDispatch(agent, prompt);
          if (validation) {
            setTaskError(validation);
            return;
          }
          try {
            const response = await controller.withLease((token) =>
              api.dispatchTask(agent.trim(), prompt.trim(), token),
            );
            setLastTaskResult(response.result);
          } catch (cause) {
            setTaskError(errorMessage(cause));
          }
        })
      }
      onActivateSkill={(activation: SkillActivation) =>
        void run(async () => {
          setSkillError(undefined);
          const validation = validateSkillActivation(activation);
          if (validation) {
            setSkillError(validation);
            return;
          }
          try {
            const result = validateDiscoveredSkill(activation);
            setLastSkillActivation(result.message);
          } catch (cause) {
            setSkillError(errorMessage(cause));
          }
        })
      }
    />
  );
}
