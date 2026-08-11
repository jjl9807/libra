"use client";

import { useState, type FormEvent } from "react";

import type { GoalStatusView } from "../../../lib/code-ui/goal-task-skill";

export interface GoalPanelProps {
  status?: GoalStatusView;
  busy?: boolean;
  error?: string;
  onStart(objective: string): void | Promise<void>;
  onRefresh(): void | Promise<void>;
  onCancel(reason: string): void | Promise<void>;
}

export function GoalPanel({
  status,
  busy = false,
  error,
  onStart,
  onRefresh,
  onCancel,
}: GoalPanelProps) {
  const [objective, setObjective] = useState("");
  const [reason, setReason] = useState("cancelled from browser");

  const submitStart = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void onStart(objective);
  };

  return (
    <section aria-label="Goal panel">
      <h2>Goal</h2>
      {status ? (
        <div>
          <p>
            {status.id ? `Goal ${status.id}` : "Goal"}
            {status.state ? ` — ${status.state}` : ""}
          </p>
          {status.objective && <p>Objective: {status.objective}</p>}
          <pre>{status.raw}</pre>
        </div>
      ) : (
        <p>No active Goal status loaded.</p>
      )}
      <form onSubmit={submitStart}>
        <label>
          Objective
          <textarea
            aria-label="Goal objective"
            value={objective}
            disabled={busy}
            onChange={(event) => setObjective(event.target.value)}
          />
        </label>
        <button type="submit" disabled={busy}>
          Start goal
        </button>
      </form>
      <button type="button" disabled={busy} onClick={() => void onRefresh()}>
        Refresh status
      </button>
      <label>
        Cancel reason
        <input
          aria-label="Goal cancel reason"
          value={reason}
          disabled={busy}
          onChange={(event) => setReason(event.target.value)}
        />
      </label>
      <button type="button" disabled={busy} onClick={() => void onCancel(reason)}>
        Cancel goal
      </button>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
