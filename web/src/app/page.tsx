"use client";

import { useMemo } from "react";

import { SessionExecutionRepair } from "@/components/workspace/execution-repair/SessionExecutionRepair";
import { SessionGoalTaskSkill } from "@/components/workspace/goal-task-skill/SessionGoalTaskSkill";
import { SessionInteractions } from "@/components/workspace/interactions/SessionInteractions";
import { SessionLifecycle } from "@/components/workspace/session-lifecycle/SessionLifecycle";
import { SessionSseResilience } from "@/components/workspace/sse-resilience/SessionSseResilience";
import { SessionUsage } from "@/components/workspace/usage/SessionUsage";
import { SessionWorkflow } from "@/components/workspace/workflow/SessionWorkflow";
import { createCodeUiClient } from "@/lib/code-ui/client";
import { BrowserControllerProvider } from "@/lib/code-ui/controller";
import { wrapClientForSseResilience } from "@/lib/code-ui/sse-resilience";
import { CodeUiStoreProvider, useCodeUiStore } from "@/lib/code-ui/store";
import { toShellViewModel } from "@/lib/code-ui/view-model";

function PlaceholderShell() {
  const { snapshot, error } = useCodeUiStore();
  const view = toShellViewModel(snapshot);
  return (
    <main style={{ maxWidth: 720, margin: "4rem auto", padding: "1.5rem" }}>
      <h1>Libra — Agent Workspace</h1>
      <p>Shared browser foundation is ready.</p>
      <p aria-live="polite">
        {error
          ? "Session unavailable"
          : snapshot
            ? `${view.title}: ${view.phase}`
            : "Loading session…"}
      </p>
      <SessionInteractions />
      <SessionGoalTaskSkill />
      <SessionLifecycle />
      <SessionUsage />
      <SessionExecutionRepair />
      <SessionWorkflow />
      <SessionSseResilience />
    </main>
  );
}

export default function Home() {
  const client = useMemo(() => wrapClientForSseResilience(createCodeUiClient()), []);
  return (
    <CodeUiStoreProvider client={client}>
      <BrowserControllerProvider>
        <PlaceholderShell />
      </BrowserControllerProvider>
    </CodeUiStoreProvider>
  );
}
