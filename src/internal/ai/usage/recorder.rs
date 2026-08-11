use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, Value};
use uuid::Uuid;

use crate::internal::ai::{
    completion::CompletionUsageSummary,
    usage::{pricing::UsagePriceTable, query::UsageQuery},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageContext {
    /// Repository identity. This is deliberately opaque: callers use the
    /// canonical `config_kv.libra.repoid`, never a mutable storage path or
    /// UI-only id.
    pub repo_id: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub agent_run_id: Option<String>,
    pub run_id: Option<String>,
    /// Runtime turn identity. It scopes current-turn usage queries.
    pub turn_id: Option<String>,
    /// Stable identity for the provider event. Replaying an event with this
    /// key is a no-op instead of charging tokens twice.
    pub event_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub request_kind: String,
    pub intent: Option<String>,
    /// Declarative agent profile name from the multi-agent runtime
    /// (`planner` / `explorer` / `reviewer` / …). `None` for the
    /// single-agent legacy path; the `agent_usage_stats` row stores
    /// NULL in that case so existing aggregation continues to match
    /// the original (provider, model) grain. See OC-Phase 5 P5.2.
    pub agent_name: Option<String>,
}

impl UsageContext {
    /// Bind a usage context to a durable runtime turn.
    ///
    /// The tool loop derives distinct per-model-request event IDs from this
    /// stable turn event ID, so retries of the same admitted runtime turn are
    /// idempotent while requests within that turn remain separately recorded.
    pub fn for_runtime_turn(&self, runtime_turn_id: &str) -> Self {
        let mut context = self.clone();
        context.turn_id = Some(runtime_turn_id.to_string());
        context.event_id = Some(format!("runtime-turn:{runtime_turn_id}"));
        context
    }

    /// Bind a cancellation to the active model request's event identity.
    ///
    /// A cancellation is only a synthetic fallback when the tool loop has
    /// not already persisted terminal usage. Sharing the active model-turn
    /// key makes the database's idempotency constraint choose one winner:
    /// either the cancellation fallback or the in-flight model request.
    pub fn for_runtime_turn_cancellation(&self, runtime_turn_id: &str, model_turn: usize) -> Self {
        let mut context = self.for_runtime_turn(runtime_turn_id);
        context.event_id = context
            .event_id
            .as_ref()
            .map(|event_id| format!("{event_id}:model-turn:{model_turn}"));
        context
    }
}

#[derive(Clone, Debug)]
pub struct UsageRecorder {
    conn: DatabaseConnection,
    pricing: UsagePriceTable,
}

impl UsageRecorder {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self {
            conn,
            pricing: UsagePriceTable::new(),
        }
    }

    pub fn with_pricing(conn: DatabaseConnection, pricing: UsagePriceTable) -> Self {
        Self { conn, pricing }
    }

    pub fn query(&self) -> UsageQuery {
        UsageQuery::new(self.conn.clone())
    }

    /// Return the repository's durable identity from its local configuration.
    ///
    /// The storage path is deliberately not a fallback: it changes when a
    /// repository moves and would split usage attribution across locations.
    pub async fn canonical_repo_id(&self) -> Result<Option<String>, DbErr> {
        let backend = self.conn.get_database_backend();
        let row = self
            .conn
            .query_one_raw(Statement::from_string(
                backend,
                "SELECT value FROM config_kv \
                 WHERE key = 'libra.repoid' \
                 ORDER BY id DESC LIMIT 1"
                    .to_string(),
            ))
            .await?;
        match row {
            Some(row) => {
                let repo_id = row.try_get_by::<String, _>("value")?;
                Ok((!repo_id.trim().is_empty()).then_some(repo_id))
            }
            None => Ok(None),
        }
    }

    pub async fn record_optional_summary(
        &self,
        context: &UsageContext,
        summary: Option<&CompletionUsageSummary>,
        wall_clock_ms: Option<u64>,
    ) -> Result<(), DbErr> {
        validate_idempotency_context(context)?;
        let Some(summary) = summary else {
            return Ok(());
        };
        self.record_summary(context, summary, wall_clock_ms).await
    }

    pub async fn record_summary(
        &self,
        context: &UsageContext,
        summary: &CompletionUsageSummary,
        wall_clock_ms: Option<u64>,
    ) -> Result<(), DbErr> {
        self.record_summary_with_tool_count(context, summary, wall_clock_ms, 0)
            .await
    }

    pub async fn record_summary_with_tool_count(
        &self,
        context: &UsageContext,
        summary: &CompletionUsageSummary,
        wall_clock_ms: Option<u64>,
        tool_call_count: u64,
    ) -> Result<(), DbErr> {
        validate_idempotency_context(context)?;
        if summary.is_zero() {
            return Ok(());
        }
        self.insert_row(UsageInsert {
            context,
            summary: Some(summary),
            wall_clock_ms: wall_clock_ms.unwrap_or(0),
            tool_call_count,
            usage_estimated: false,
            success: true,
            error_kind: None,
        })
        .await
    }

    pub async fn record_missing_usage(
        &self,
        context: &UsageContext,
        wall_clock_ms: Option<u64>,
        tool_call_count: u64,
    ) -> Result<(), DbErr> {
        validate_idempotency_context(context)?;
        self.insert_row(UsageInsert {
            context,
            summary: None,
            wall_clock_ms: wall_clock_ms.unwrap_or(0),
            tool_call_count,
            usage_estimated: true,
            success: true,
            error_kind: None,
        })
        .await
    }

    pub async fn record_failure(
        &self,
        context: &UsageContext,
        error_kind: &str,
        wall_clock_ms: Option<u64>,
    ) -> Result<(), DbErr> {
        validate_idempotency_context(context)?;
        self.insert_row(UsageInsert {
            context,
            summary: None,
            wall_clock_ms: wall_clock_ms.unwrap_or(0),
            tool_call_count: 0,
            usage_estimated: false,
            success: false,
            error_kind: Some(error_kind),
        })
        .await
    }

    pub async fn prune_before(&self, cutoff_rfc3339: &str) -> Result<u64, DbErr> {
        let backend = self.conn.get_database_backend();
        let result = self
            .conn
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "DELETE FROM agent_usage_stats \
                 WHERE COALESCE(started_at, created_at) < ?",
                vec![cutoff_rfc3339.to_string().into()],
            ))
            .await?;
        Ok(result.rows_affected())
    }

    async fn insert_row(&self, input: UsageInsert<'_>) -> Result<(), DbErr> {
        let now = Utc::now().to_rfc3339();
        let summary = input.summary.cloned().unwrap_or_default();
        let total_tokens = summary.total_tokens.unwrap_or_else(|| {
            summary
                .input_tokens
                .saturating_add(summary.output_tokens)
                .saturating_add(summary.reasoning_tokens.unwrap_or(0))
        });
        // A missing provider usage payload is not a zero-token request. Do
        // not manufacture a `$0` estimate from the default summary; callers
        // must receive an explicit unknown cost state.
        let cost_micro_dollars = input.summary.and_then(|summary| {
            if summary.cost_usd.is_some() {
                None
            } else {
                self.pricing.estimate_micro_dollars(
                    &input.context.provider,
                    &input.context.model,
                    summary,
                )
            }
        });
        let backend = self.conn.get_database_backend();
        self.conn
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "INSERT INTO agent_usage_stats \
                 (id, repo_id, session_id, thread_id, agent_run_id, run_id, turn_id, event_id, provider, model, agent_name, request_kind, intent, prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens, total_tokens, tool_call_count, wall_clock_ms, provider_latency_ms, cost_estimate_micro_dollars, cost_usd, usage_estimated, started_at, finished_at, success, error_kind, schema_version, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(session_id, event_id) DO UPDATE SET \
                    prompt_tokens = excluded.prompt_tokens, \
                    completion_tokens = excluded.completion_tokens, \
                    cached_tokens = excluded.cached_tokens, \
                    reasoning_tokens = excluded.reasoning_tokens, \
                    total_tokens = excluded.total_tokens, \
                    tool_call_count = excluded.tool_call_count, \
                    wall_clock_ms = excluded.wall_clock_ms, \
                    provider_latency_ms = excluded.provider_latency_ms, \
                    cost_estimate_micro_dollars = excluded.cost_estimate_micro_dollars, \
                    cost_usd = excluded.cost_usd, \
                    usage_estimated = excluded.usage_estimated, \
                    started_at = excluded.started_at, \
                    finished_at = excluded.finished_at, \
                    success = excluded.success, \
                    error_kind = excluded.error_kind, \
                    schema_version = excluded.schema_version, \
                    created_at = excluded.created_at \
                 WHERE excluded.success = 1 \
                   AND excluded.usage_estimated = 0 \
                   AND agent_usage_stats.success = 0 \
                   AND agent_usage_stats.usage_estimated = 0",
                vec![
                    Uuid::new_v4().to_string().into(),
                    input.context.repo_id.clone().into(),
                    input.context.session_id.clone().into(),
                    input.context.thread_id.clone().into(),
                    input.context.agent_run_id.clone().into(),
                    input.context.run_id.clone().into(),
                    input.context.turn_id.clone().into(),
                    input.context.event_id.clone().into(),
                    input.context.provider.clone().into(),
                    input.context.model.clone().into(),
                    input.context.agent_name.clone().into(),
                    input.context.request_kind.clone().into(),
                    input.context.intent.clone().into(),
                    u64_to_i64_value(summary.input_tokens),
                    u64_to_i64_value(summary.output_tokens),
                    u64_to_i64_value(summary.cached_tokens.unwrap_or(0)),
                    u64_to_i64_value(summary.reasoning_tokens.unwrap_or(0)),
                    u64_to_i64_value(total_tokens),
                    u64_to_i64_value(input.tool_call_count),
                    u64_to_i64_value(input.wall_clock_ms),
                    Value::BigInt(None),
                    optional_i64_value(cost_micro_dollars),
                    summary.cost_usd.into(),
                    bool_to_i64_value(input.usage_estimated),
                    now.clone().into(),
                    now.clone().into(),
                    bool_to_i64_value(input.success),
                    input.error_kind.map(str::to_string).into(),
                    1_i64.into(),
                    now.into(),
                ],
            ))
            .await?;
        Ok(())
    }
}

struct UsageInsert<'a> {
    context: &'a UsageContext,
    summary: Option<&'a CompletionUsageSummary>,
    wall_clock_ms: u64,
    tool_call_count: u64,
    usage_estimated: bool,
    success: bool,
    error_kind: Option<&'a str>,
}

fn u64_to_i64_value(value: u64) -> Value {
    i64::try_from(value).unwrap_or(i64::MAX).into()
}

fn optional_i64_value(value: Option<i64>) -> Value {
    value.into()
}

fn bool_to_i64_value(value: bool) -> Value {
    i64::from(value).into()
}

fn validate_idempotency_context(context: &UsageContext) -> Result<(), DbErr> {
    if context.event_id.is_some()
        && context
            .session_id
            .as_deref()
            .is_none_or(|session_id| session_id.trim().is_empty())
    {
        return Err(DbErr::Custom(
            "usage event_id requires a non-empty session_id for idempotency".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `u64_to_i64_value` must clamp values exceeding `i64::MAX` to
    /// `i64::MAX` rather than wrapping or panicking. Pin both the
    /// happy path (`u64::MAX -> i64::MAX`) and a representative
    /// in-range value.
    #[test]
    fn u64_to_i64_value_clamps_overflow_to_i64_max() {
        // In-range value passes through unchanged.
        match u64_to_i64_value(42) {
            Value::BigInt(Some(v)) => assert_eq!(v, 42),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
        match u64_to_i64_value(0) {
            Value::BigInt(Some(v)) => assert_eq!(v, 0),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
        // i64::MAX is the boundary — exactly representable.
        match u64_to_i64_value(i64::MAX as u64) {
            Value::BigInt(Some(v)) => assert_eq!(v, i64::MAX),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
        // u64::MAX overflows -> clamped to i64::MAX.
        match u64_to_i64_value(u64::MAX) {
            Value::BigInt(Some(v)) => assert_eq!(v, i64::MAX),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
    }

    /// `bool_to_i64_value` maps `true -> 1` and `false -> 0`. Pin
    /// against a future "true→-1" sentinel encoding refactor.
    #[test]
    fn bool_to_i64_value_maps_true_one_false_zero() {
        match bool_to_i64_value(true) {
            Value::BigInt(Some(v)) => assert_eq!(v, 1),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
        match bool_to_i64_value(false) {
            Value::BigInt(Some(v)) => assert_eq!(v, 0),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
    }

    /// `optional_i64_value` round-trips both Some and None.
    #[test]
    fn optional_i64_value_threads_some_and_none() {
        match optional_i64_value(Some(42)) {
            Value::BigInt(Some(v)) => assert_eq!(v, 42),
            other => panic!("expected BigInt(Some), got {other:?}"),
        }
        match optional_i64_value(None) {
            Value::BigInt(None) => {}
            other => panic!("expected BigInt(None), got {other:?}"),
        }
    }

    /// `UsageContext` clones cleanly (the recorder clones the context
    /// per insert to thread fields into the SQL statement).
    #[test]
    fn usage_context_derives_clone_and_eq() {
        let ctx = UsageContext {
            repo_id: Some("repo-1".to_string()),
            session_id: Some("s1".to_string()),
            thread_id: Some("t1".to_string()),
            agent_run_id: Some("r1".to_string()),
            run_id: Some("run1".to_string()),
            turn_id: Some("turn1".to_string()),
            event_id: Some("event1".to_string()),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            request_kind: "chat".to_string(),
            intent: Some("fix".to_string()),
            agent_name: Some("coder".to_string()),
        };
        let cloned = ctx.clone();
        assert_eq!(cloned, ctx);
        // Subtle: agent_name=None must be distinguishable from
        // agent_name=Some("") for the per-agent grouping path.
        let mut anon = ctx.clone();
        anon.agent_name = None;
        assert_ne!(anon, ctx);
        let mut empty = ctx.clone();
        empty.agent_name = Some(String::new());
        assert_ne!(empty, ctx);
        assert_ne!(empty.agent_name, anon.agent_name);
    }

    #[test]
    fn usage_context_binds_durable_runtime_turn_ids() {
        let context = UsageContext {
            repo_id: Some("repo-1".to_string()),
            session_id: Some("session-1".to_string()),
            thread_id: None,
            agent_run_id: None,
            run_id: None,
            turn_id: None,
            event_id: None,
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            request_kind: "completion".to_string(),
            intent: None,
            agent_name: None,
        };

        let bound = context.for_runtime_turn("tui-local-123");
        assert_eq!(bound.turn_id.as_deref(), Some("tui-local-123"));
        assert_eq!(
            bound.event_id.as_deref(),
            Some("runtime-turn:tui-local-123")
        );
        assert_eq!(context.turn_id, None);
        assert_eq!(context.event_id, None);
    }

    #[test]
    fn event_id_requires_non_empty_session_id() {
        let context = UsageContext {
            repo_id: None,
            session_id: None,
            thread_id: None,
            agent_run_id: None,
            run_id: None,
            turn_id: None,
            event_id: Some("event-1".to_string()),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            request_kind: "completion".to_string(),
            intent: None,
            agent_name: None,
        };

        assert!(
            validate_idempotency_context(&context)
                .expect_err("event ids require a session")
                .to_string()
                .contains("usage event_id requires a non-empty session_id"),
            "the recorder must reject event IDs without an idempotency scope"
        );
    }
}
