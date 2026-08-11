"use client";

import type { SubAgentUsageRow } from "../../../lib/code-ui/usage";

import { UsageTotalsPanel } from "./UsageTotalsPanel";

export interface SubAgentUsageListProps {
  rows: SubAgentUsageRow[];
}

export function SubAgentUsageList({ rows }: SubAgentUsageListProps) {
  if (rows.length === 0) {
    return (
      <section aria-label="Sub-agent usage">
        <h3>Sub-agent attribution</h3>
        <p>No sub-agent usage rows.</p>
      </section>
    );
  }

  return (
    <section aria-label="Sub-agent usage">
      <h3>Sub-agent attribution</h3>
      <ul>
        {rows.map((row) => {
          const label = row.agentName?.trim() || row.agentRunId || "sub-agent";
          return (
            <li key={`${row.agentRunId ?? ""}:${row.agentName ?? ""}:${label}`}>
              <UsageTotalsPanel title={`Sub-agent: ${label}`} totals={row.totals} />
            </li>
          );
        })}
      </ul>
    </section>
  );
}
