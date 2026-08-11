"use client";

import type { UsageReadModel } from "../../../lib/code-ui/usage";

import { SubAgentUsageList } from "./SubAgentUsageList";
import { UsageTotalsPanel } from "./UsageTotalsPanel";

export interface UsageHostProps {
  model?: UsageReadModel;
  busy?: boolean;
  error?: string;
  deferredHint?: string;
  onRefresh(): void | Promise<void>;
}

export function UsageHost({
  model,
  busy = false,
  error,
  deferredHint,
  onRefresh,
}: UsageHostProps) {
  return (
    <div aria-label="Usage workspace">
      <h2>Usage</h2>
      {deferredHint ? <p>{deferredHint}</p> : null}
      {model?.sessionId ? <p>Session: {model.sessionId}</p> : null}
      {model?.turnId ? <p>Turn: {model.turnId}</p> : null}
      <button type="button" disabled={busy} onClick={() => void onRefresh()}>
        Refresh usage
      </button>
      <UsageTotalsPanel title="Cumulative" totals={model?.cumulative} />
      <UsageTotalsPanel
        title="Current turn"
        totals={model?.turnDelta}
        emptyLabel="No current-turn delta loaded."
      />
      <SubAgentUsageList
        rows={model?.subAgents}
        status={model?.subAgentsStatus}
      />
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}

export { SubAgentUsageList } from "./SubAgentUsageList";
export { UsageTotalsPanel } from "./UsageTotalsPanel";
