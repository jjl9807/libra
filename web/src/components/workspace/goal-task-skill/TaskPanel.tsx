"use client";

import { useState, type FormEvent } from "react";

import type { CodeUiTaskSnapshot } from "../../../lib/code-ui/types";
import { sortTasks } from "../../../lib/code-ui/goal-task-skill";

export interface TaskPanelProps {
  tasks: CodeUiTaskSnapshot[];
  busy?: boolean;
  error?: string;
  lastResult?: string;
  onDispatch(agent: string, prompt: string): void | Promise<void>;
}

export function TaskPanel({
  tasks,
  busy = false,
  error,
  lastResult,
  onDispatch,
}: TaskPanelProps) {
  const [agent, setAgent] = useState("");
  const [prompt, setPrompt] = useState("");
  const ordered = sortTasks(tasks);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void onDispatch(agent, prompt);
  };

  return (
    <section aria-label="Task panel">
      <h2>Tasks</h2>
      {ordered.length === 0 ? (
        <p>No tasks in the current session snapshot.</p>
      ) : (
        <ul>
          {ordered.map((task) => (
            <li key={task.id}>
              <strong>{task.title ?? task.id}</strong> — {task.status}
              {task.details ? `: ${task.details}` : ""}
            </li>
          ))}
        </ul>
      )}
      <form onSubmit={submit}>
        <label>
          Agent
          <input
            aria-label="Task agent"
            value={agent}
            disabled={busy}
            onChange={(event) => setAgent(event.target.value)}
          />
        </label>
        <label>
          Prompt
          <textarea
            aria-label="Task prompt"
            value={prompt}
            disabled={busy}
            onChange={(event) => setPrompt(event.target.value)}
          />
        </label>
        <button type="submit" disabled={busy}>
          Dispatch task
        </button>
      </form>
      {lastResult && <p role="status">{lastResult}</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
