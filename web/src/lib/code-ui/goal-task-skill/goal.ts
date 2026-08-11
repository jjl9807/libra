import type { CodeUiApiError, CodeUiTaskSnapshot } from "../types";

export interface GoalStatusView {
  raw: string;
  id?: string;
  state?: string;
  objective?: string;
}

export interface GoalCommandResult {
  accepted: boolean;
  status: string;
}

export interface TaskDispatchResult {
  accepted: boolean;
  result: string;
}

export interface GoalTaskSkillTransport {
  request<T>(path: string, init?: RequestInit): Promise<T>;
}

function headers(token?: string): HeadersInit {
  return {
    "content-type": "application/json",
    ...(token ? { "X-Code-Controller-Token": token } : {}),
  };
}

/** Parse the human-readable `render_goal_status` text into a compact view model. */
export function parseGoalStatus(raw: string): GoalStatusView {
  const text = raw.trim();
  const view: GoalStatusView = { raw: text };
  const header = text.match(/^Goal\s+(\S+)\s+[—-]\s+(.+)$/m);
  if (header) {
    view.id = header[1];
    view.state = header[2]?.trim();
  }
  const objective = text.match(/^Objective:\s*(.+)$/m);
  if (objective) view.objective = objective[1]?.trim();
  return view;
}

export function validateObjective(objective: string): string | undefined {
  if (!objective.trim()) return "Enter a Goal objective before starting.";
  // Mirror `MAX_OBJECTIVE_LEN` (16 KiB UTF-8) from `internal/ai/goal/spec.rs`.
  if (new TextEncoder().encode(objective).byteLength > 16 * 1024) {
    return "Goal objective must be 16 KiB or smaller.";
  }
  return undefined;
}

/** True when `/goal/status` (or cancel) reports the designated empty Goal state. */
export function isAbsentGoalError(cause: unknown): boolean {
  if (!cause || typeof cause !== "object") return false;
  const code = "code" in cause && typeof cause.code === "string" ? cause.code : "";
  if (code === "GOAL_NOT_ACTIVE") return true;
  const message =
    "message" in cause && typeof cause.message === "string" ? cause.message.toLowerCase() : "";
  return message.includes("no active goal") || message.includes("no goal is active");
}

export function validateTaskDispatch(agent: string, prompt: string): string | undefined {
  if (!agent.trim()) return "Enter an agent name before dispatching a task.";
  if (!prompt.trim()) return "Enter a task prompt before dispatching.";
  return undefined;
}

export function sortTasks(tasks: CodeUiTaskSnapshot[]): CodeUiTaskSnapshot[] {
  return [...tasks].sort((left, right) => right.updatedAt.localeCompare(left.updatedAt));
}

export class FetchGoalTaskSkillTransport implements GoalTaskSkillTransport {
  constructor(private readonly baseUrl = "") {}

  async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      credentials: "same-origin",
      ...init,
    });
    if (!response.ok) {
      let message = response.statusText;
      let code: string | undefined;
      try {
        const body = (await response.json()) as { error?: Partial<CodeUiApiError> };
        if (typeof body.error?.message === "string") message = body.error.message;
        if (typeof body.error?.code === "string") code = body.error.code;
      } catch {
        // Non-JSON failures still surface statusText.
      }
      throw { status: response.status, message, code } satisfies CodeUiApiError;
    }
    return (await response.json()) as T;
  }
}

export function createGoalTaskSkillApi(
  transport: GoalTaskSkillTransport = new FetchGoalTaskSkillTransport(),
) {
  return {
    startGoal(objective: string, token: string): Promise<GoalCommandResult> {
      return transport.request("/api/code/goal/start", {
        method: "POST",
        headers: headers(token),
        body: JSON.stringify({ objective }),
      });
    },
    goalStatus(): Promise<{ status: string }> {
      return transport.request("/api/code/goal/status");
    },
    cancelGoal(reason: string, token: string): Promise<GoalCommandResult> {
      return transport.request("/api/code/goal/cancel", {
        method: "POST",
        headers: headers(token),
        body: JSON.stringify({ reason }),
      });
    },
    dispatchTask(agent: string, prompt: string, token: string): Promise<TaskDispatchResult> {
      return transport.request("/api/code/task/dispatch", {
        method: "POST",
        headers: headers(token),
        body: JSON.stringify({ agent, prompt }),
      });
    },
    activateSkill(
      activation: { provider: string; name: string },
      token: string,
    ): Promise<{ accepted: true; effect?: string; activated?: boolean; message?: string }> {
      return transport.request("/api/code/skills/activate", {
        method: "POST",
        headers: headers(token),
        body: JSON.stringify(activation),
      });
    },
  };
}

export type GoalTaskSkillApi = ReturnType<typeof createGoalTaskSkillApi>;
