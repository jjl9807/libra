"use client";

import { SessionExecutionRepair } from "@/components/workspace/execution-repair/SessionExecutionRepair";
import { SessionGoalTaskSkill } from "@/components/workspace/goal-task-skill/SessionGoalTaskSkill";
import { SessionInteractions } from "@/components/workspace/interactions/SessionInteractions";
import { SessionLifecycle } from "@/components/workspace/session-lifecycle/SessionLifecycle";
import { SessionUsage } from "@/components/workspace/usage/SessionUsage";
import { BrowserControllerProvider } from "@/lib/code-ui/controller";
import { CodeUiStoreProvider, useCodeUiStore } from "@/lib/code-ui/store";
import { toShellViewModel } from "@/lib/code-ui/view-model";

function PlaceholderShell() {
  const { snapshot, error } = useCodeUiStore();
  const view = toShellViewModel(snapshot);
  return (
    <main style={{ maxWidth: 720, margin: "4rem auto", padding: "1.5rem" }}>
      <h1>Libra Code</h1>
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
    </main>
  );
}

export default function Home() {
  return (
    <CodeUiStoreProvider>
      <BrowserControllerProvider>
        <PlaceholderShell />
      </BrowserControllerProvider>
    </CodeUiStoreProvider>
  );
}
