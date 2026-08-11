//! UI-neutral usage read model for agent runtimes.
//!
//! This module intentionally projects the shared `ai::usage` store. Browser
//! and terminal consumers get identical totals and uncertainty semantics.

use sea_orm::DbErr;

use crate::internal::ai::usage::{UsageAggregate, UsageQueryFilter, UsageRecorder};

/// Whether a displayed usage dimension is complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageStatus {
    Known,
    Partial,
    Unknown,
}

impl UsageStatus {
    /// Stable lowercase label for human-readable and CSV usage reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeUsageTotals {
    pub request_count: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_estimate_micro_dollars: Option<u64>,
    pub usage_status: UsageStatus,
    pub cost_status: UsageStatus,
    pub error_status: UsageStatus,
    pub failed_count: u64,
    pub unknown_usage_count: u64,
    pub unknown_cost_count: u64,
}

#[derive(Clone)]
pub struct RuntimeUsageService {
    recorder: UsageRecorder,
}

impl RuntimeUsageService {
    pub fn new(recorder: UsageRecorder) -> Self {
        Self { recorder }
    }

    /// Cumulative totals for the supplied repository/session filter.
    pub async fn cumulative(&self, filter: UsageQueryFilter) -> Result<RuntimeUsageTotals, DbErr> {
        self.totals(filter).await
    }

    /// Totals belonging to exactly one runtime turn. A runtime turn id is
    /// mandatory so callers cannot accidentally display session totals as a
    /// current-turn delta.
    pub async fn current_turn(
        &self,
        turn_id: impl Into<String>,
        mut filter: UsageQueryFilter,
    ) -> Result<RuntimeUsageTotals, DbErr> {
        filter.turn_id = Some(turn_id.into());
        self.totals(filter).await
    }

    /// Child-agent breakdown using the same persisted rows as all other
    /// runtime callers. `agent_run_id` is optional to support agent-name views.
    pub async fn sub_agent(
        &self,
        agent_run_id: Option<String>,
        agent_name: Option<String>,
        mut filter: UsageQueryFilter,
    ) -> Result<RuntimeUsageTotals, DbErr> {
        filter.agent_run_id = agent_run_id;
        filter.agent_name = agent_name;
        self.totals(filter).await
    }

    async fn totals(&self, filter: UsageQueryFilter) -> Result<RuntimeUsageTotals, DbErr> {
        let rows = self
            .recorder
            .query()
            .aggregate_by_model_filtered(&filter)
            .await?;
        Ok(fold(rows))
    }
}

fn fold(rows: Vec<UsageAggregate>) -> RuntimeUsageTotals {
    let request_count = rows.iter().map(|row| row.request_count).sum();
    let total_tokens = rows.iter().map(|row| row.total_tokens).sum();
    let failed_count = rows.iter().map(|row| row.failed_count).sum();
    let unknown_usage_count = rows.iter().map(|row| row.unknown_usage_count).sum();
    let unknown_cost_count = rows.iter().map(|row| row.unknown_cost_count).sum();
    let cost_usd = sum_present(rows.iter().map(|row| row.cost_usd));
    let cost_estimate_micro_dollars =
        sum_present(rows.iter().map(|row| row.cost_estimate_micro_dollars));

    RuntimeUsageTotals {
        request_count,
        total_tokens,
        cost_usd,
        cost_estimate_micro_dollars,
        usage_status: usage_status(request_count, unknown_usage_count),
        cost_status: usage_status(request_count, unknown_cost_count),
        error_status: if failed_count == 0 {
            UsageStatus::Known
        } else if failed_count == request_count {
            UsageStatus::Unknown
        } else {
            UsageStatus::Partial
        },
        failed_count,
        unknown_usage_count,
        unknown_cost_count,
    }
}

/// Sum the known values without discarding them when another model has none.
fn sum_present<T>(values: impl Iterator<Item = Option<T>>) -> Option<T>
where
    T: std::iter::Sum,
{
    let mut known_values = values.flatten().peekable();
    known_values.peek()?;
    Some(known_values.sum())
}

/// Classify a usage dimension from its request and unknown-row counts.
///
/// A partial value is a known subtotal, not an exact aggregate.
pub fn usage_status(total: u64, unknown: u64) -> UsageStatus {
    if total == 0 || unknown == total {
        UsageStatus::Unknown
    } else if unknown == 0 {
        UsageStatus::Known
    } else {
        UsageStatus::Partial
    }
}
