"use client";

import {
  formatCost,
  formatErrorStatus,
  formatTokens,
  isIncompleteUsage,
  type RuntimeUsageTotals,
} from "../../../lib/code-ui/usage";

export interface UsageTotalsPanelProps {
  title: string;
  totals?: RuntimeUsageTotals;
  emptyLabel?: string;
}

export function UsageTotalsPanel({
  title,
  totals,
  emptyLabel = "No usage totals loaded.",
}: UsageTotalsPanelProps) {
  if (!totals) {
    return (
      <section aria-label={title}>
        <h3>{title}</h3>
        <p>{emptyLabel}</p>
      </section>
    );
  }

  return (
    <section aria-label={title}>
      <h3>{title}</h3>
      <dl>
        <div>
          <dt>Requests</dt>
          <dd>{totals.requestCount}</dd>
        </div>
        <div>
          <dt>Tokens</dt>
          <dd>{formatTokens(totals)}</dd>
        </div>
        <div>
          <dt>Cost</dt>
          <dd>{formatCost(totals)}</dd>
        </div>
        <div>
          <dt>Errors</dt>
          <dd>{formatErrorStatus(totals)}</dd>
        </div>
        <div>
          <dt>Usage status</dt>
          <dd>{totals.usageStatus}</dd>
        </div>
        <div>
          <dt>Cost status</dt>
          <dd>{totals.costStatus}</dd>
        </div>
      </dl>
      {isIncompleteUsage(totals) ? (
        <p role="status">
          Incomplete usage — do not treat missing provider data as zero spend
          (unknown_usage={totals.unknownUsageCount}, unknown_cost={totals.unknownCostCount}).
        </p>
      ) : null}
    </section>
  );
}
