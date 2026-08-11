"use client";

import type { CodeUiPlanSnapshot, CodeUiToolCallSnapshot } from "../../../lib/code-ui/types";

export interface ExecutionProgressPanelProps {
  plans: CodeUiPlanSnapshot[];
  toolCalls: CodeUiToolCallSnapshot[];
}

export function ExecutionProgressPanel({ plans, toolCalls }: ExecutionProgressPanelProps) {
  return (
    <section aria-label="Execution progress panel">
      <h2>Execution</h2>
      {plans.length === 0 && toolCalls.length === 0 ? (
        <p>No confirmed plan execution in the current snapshot.</p>
      ) : null}
      {plans.map((plan) => (
        <div key={plan.id}>
          <h3>{plan.title?.trim() || plan.id}</h3>
          <p>Status: {plan.status}</p>
          {plan.summary ? <p>{plan.summary}</p> : null}
          <ul aria-label={`Plan steps for ${plan.id}`}>
            {plan.steps.map((step) => (
              <li key={`${plan.id}:${step.step}`}>
                {step.step} — {step.status}
              </li>
            ))}
          </ul>
        </div>
      ))}
      {toolCalls.length > 0 ? (
        <ul aria-label="Tool calls">
          {toolCalls.map((call) => (
            <li key={call.id}>
              {call.toolName} — {call.status}
              {call.summary ? `: ${call.summary}` : ""}
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
