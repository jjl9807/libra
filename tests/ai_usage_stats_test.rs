//! CEX-16 usage stats persistence and aggregation tests.

use libra::internal::{
    ai::{
        agent::runtime::{RuntimeUsageService, UsageStatus},
        completion::CompletionUsageSummary,
        usage::{
            UsageContext, UsagePrice, UsagePriceTable, UsageQuery, UsageQueryFilter, UsageRecorder,
        },
    },
    db::migration::run_builtin_migrations,
};
use sea_orm::{ConnectionTrait, Database, Statement};

fn usage_context(provider: &str, model: &str) -> UsageContext {
    UsageContext {
        repo_id: Some("repo-1".to_string()),
        session_id: Some("session-1".to_string()),
        thread_id: Some("thread-1".to_string()),
        agent_run_id: None,
        run_id: Some("run-1".to_string()),
        turn_id: Some("turn-1".to_string()),
        event_id: None,
        provider: provider.to_string(),
        model: model.to_string(),
        request_kind: "completion".to_string(),
        intent: Some("feature".to_string()),
        agent_name: None,
    }
}

fn usage_context_with_agent(provider: &str, model: &str, agent: &str) -> UsageContext {
    UsageContext {
        agent_name: Some(agent.to_string()),
        ..usage_context(provider, model)
    }
}

#[tokio::test]
async fn usage_recorder_persists_and_aggregates_by_model() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-test");

    recorder
        .record_summary_with_tool_count(
            &context,
            &CompletionUsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cached_tokens: Some(2),
                reasoning_tokens: Some(1),
                total_tokens: Some(16),
                cost_usd: Some(0.25),
            },
            Some(1200),
            2,
        )
        .await
        .expect("record first usage");
    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 7,
                output_tokens: 3,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(10),
                cost_usd: None,
            },
            Some(800),
        )
        .await
        .expect("record second usage");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider, "openai");
    assert_eq!(rows[0].model, "gpt-test");
    assert_eq!(rows[0].request_count, 2);
    assert_eq!(rows[0].prompt_tokens, 17);
    assert_eq!(rows[0].completion_tokens, 8);
    assert_eq!(rows[0].cached_tokens, 2);
    assert_eq!(rows[0].reasoning_tokens, 1);
    assert_eq!(rows[0].total_tokens, 26);
    assert_eq!(rows[0].tool_call_count, 2);
    assert_eq!(rows[0].wall_clock_ms, 2000);
    assert_eq!(rows[0].cost_usd, Some(0.25));
    assert_eq!(rows[0].cost_estimate_micro_dollars, None);
    assert_eq!(rows[0].failed_count, 0);
}

#[tokio::test]
async fn usage_recorder_ignores_absent_or_zero_usage() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("ollama", "local-test");

    recorder
        .record_optional_summary(&context, None, Some(40))
        .await
        .expect("missing usage is tolerated");
    recorder
        .record_summary(&context, &CompletionUsageSummary::default(), Some(40))
        .await
        .expect("zero usage is tolerated");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");
    assert!(rows.is_empty());
}

#[tokio::test]
async fn usage_recorder_estimates_cost_from_builtin_price_table() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-4o-mini");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 1_000_000,
                output_tokens: 2_000_000,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(3_000_000),
                cost_usd: None,
            },
            Some(100),
        )
        .await
        .expect("record estimated cost usage");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");

    assert_eq!(rows[0].cost_usd, None);
    assert_eq!(rows[0].cost_estimate_micro_dollars, Some(1_350_000));
}

#[tokio::test]
async fn usage_query_preserves_exact_and_estimated_costs_in_one_aggregate() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-4o-mini");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(2),
                cost_usd: Some(0.25),
            },
            Some(100),
        )
        .await
        .expect("record exact cost");
    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 1_000_000,
                output_tokens: 2_000_000,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(3_000_000),
                cost_usd: None,
            },
            Some(100),
        )
        .await
        .expect("record estimated cost");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate mixed costs");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cost_usd, Some(0.25));
    assert_eq!(rows[0].cost_estimate_micro_dollars, Some(1_350_000));
    assert_eq!(rows[0].unknown_cost_count, 0);
}

#[tokio::test]
async fn usage_recorder_allows_project_price_overrides() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let pricing = UsagePriceTable::new().with_override(
        "custom",
        "model",
        UsagePrice::new(10, 20)
            .with_cached_micro_dollars_per_mtok(2)
            .with_reasoning_micro_dollars_per_mtok(30),
    );
    let recorder = UsageRecorder::with_pricing(conn.clone(), pricing);
    let context = usage_context("custom", "model");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 2_000_000,
                output_tokens: 1_000_000,
                cached_tokens: Some(500_000),
                reasoning_tokens: Some(1_000_000),
                total_tokens: Some(4_000_000),
                cost_usd: None,
            },
            Some(100),
        )
        .await
        .expect("record override cost usage");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");

    assert_eq!(rows[0].cost_estimate_micro_dollars, Some(66));
}

#[tokio::test]
async fn usage_recorder_records_missing_usage_and_failures() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("gemini", "gemini-test");

    recorder
        .record_missing_usage(&context, Some(250), 1)
        .await
        .expect("record estimated zero-token usage");
    recorder
        .record_failure(&context, "provider_error", Some(750))
        .await
        .expect("record failed usage");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].request_count, 2);
    assert_eq!(rows[0].total_tokens, 0);
    assert_eq!(rows[0].tool_call_count, 1);
    assert_eq!(rows[0].wall_clock_ms, 1000);
    assert_eq!(rows[0].failed_count, 1);
    assert_eq!(rows[0].unknown_usage_count, 2);
}

#[tokio::test]
async fn usage_query_filter_excludes_failures_by_default_when_requested() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-test");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 4,
                output_tokens: 6,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(10),
                cost_usd: None,
            },
            Some(100),
        )
        .await
        .expect("record success");
    recorder
        .record_failure(&context, "provider_error", Some(900))
        .await
        .expect("record failure");

    let success_rows = UsageQuery::new(conn.clone())
        .aggregate_by_model_filtered(&UsageQueryFilter {
            include_failed: false,
            ..UsageQueryFilter::default()
        })
        .await
        .expect("aggregate successes");
    assert_eq!(success_rows[0].request_count, 1);
    assert_eq!(success_rows[0].failed_count, 0);
    assert_eq!(success_rows[0].wall_clock_ms, 100);

    let all_rows = UsageQuery::new(conn)
        .aggregate_by_model_filtered(&UsageQueryFilter {
            include_failed: true,
            ..UsageQueryFilter::default()
        })
        .await
        .expect("aggregate all rows");
    assert_eq!(all_rows[0].request_count, 2);
    assert_eq!(all_rows[0].failed_count, 1);
    assert_eq!(all_rows[0].wall_clock_ms, 1000);
}

#[tokio::test]
async fn usage_recorder_prunes_rows_before_cutoff() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-test");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(2),
                cost_usd: None,
            },
            Some(10),
        )
        .await
        .expect("record usage");

    conn.execute_raw(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE agent_usage_stats SET started_at = ?, created_at = ?",
        vec![
            "2020-01-01T00:00:00+00:00".into(),
            "2020-01-01T00:00:00+00:00".into(),
        ],
    ))
    .await
    .expect("age usage row");

    let deleted = recorder
        .prune_before("2021-01-01T00:00:00+00:00")
        .await
        .expect("prune old rows");
    assert_eq!(deleted, 1);

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");
    assert!(rows.is_empty());
}

/// OC-Phase 5 P5.2: the recorder persists `agent_name` and the
/// query layer can aggregate at three documented grains:
/// `(provider, model)` (legacy), `(agent_name)`, and
/// `(agent_name, provider, model)`. Mixing rows with and without
/// `agent_name` exercises the legacy back-compat path: the
/// `(provider, model)` aggregation collapses every row regardless of
/// agent, while the `(agent_name, provider, model)` aggregation
/// surfaces the agent dimension and keeps `agent_name = None` for
/// the legacy row.
#[tokio::test]
async fn usage_query_aggregates_by_agent_name_grouping() {
    use libra::internal::ai::usage::UsageGrouping;

    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());

    let summary = CompletionUsageSummary {
        input_tokens: 4,
        output_tokens: 2,
        cached_tokens: None,
        reasoning_tokens: None,
        total_tokens: Some(6),
        cost_usd: Some(0.10),
    };

    // Two `planner` rows on the same model — should fold into one
    // row of the agent-grain aggregation.
    let planner = usage_context_with_agent("openai", "gpt-4o", "planner");
    recorder
        .record_summary(&planner, &summary, Some(100))
        .await
        .expect("planner row 1");
    recorder
        .record_summary(&planner, &summary, Some(150))
        .await
        .expect("planner row 2");

    // One `explorer` row on a different model.
    let explorer = usage_context_with_agent("deepseek", "deepseek-chat", "explorer");
    recorder
        .record_summary(&explorer, &summary, Some(200))
        .await
        .expect("explorer row");

    // One legacy single-agent row (agent_name = None).
    let legacy = usage_context("openai", "gpt-4o");
    recorder
        .record_summary(&legacy, &summary, Some(50))
        .await
        .expect("legacy row");

    let query = UsageQuery::new(conn.clone());

    // Grain 1: legacy (provider, model). All four rows fold into two
    // groups; agent_name is None on every result.
    let by_pm = query
        .aggregate_filtered(UsageGrouping::ProviderModel, &UsageQueryFilter::default())
        .await
        .expect("by-provider-model");
    assert_eq!(by_pm.len(), 2, "two (provider, model) groups");
    assert!(by_pm.iter().all(|r| r.agent_name.is_none()));
    let openai_pm = by_pm
        .iter()
        .find(|r| r.provider == "openai" && r.model == "gpt-4o")
        .expect("openai/gpt-4o group");
    // 2 planner rows + 1 legacy row = 3 requests.
    assert_eq!(openai_pm.request_count, 3);

    // Grain 2: agent only. Three groups: planner / explorer / legacy(None).
    let by_agent = query
        .aggregate_filtered(UsageGrouping::Agent, &UsageQueryFilter::default())
        .await
        .expect("by-agent");
    assert_eq!(by_agent.len(), 3);
    let planner_total = by_agent
        .iter()
        .find(|r| r.agent_name.as_deref() == Some("planner"))
        .expect("planner group");
    assert_eq!(planner_total.request_count, 2);
    let legacy_total = by_agent
        .iter()
        .find(|r| r.agent_name.is_none())
        .expect("legacy group");
    assert_eq!(legacy_total.request_count, 1);
    assert!(legacy_total.provider.is_empty());

    // Grain 3: full (agent_name, provider, model). Three groups
    // because (planner, openai, gpt-4o) folds two rows; the others
    // each contribute one row.
    let by_apm = query
        .aggregate_filtered(
            UsageGrouping::AgentProviderModel,
            &UsageQueryFilter::default(),
        )
        .await
        .expect("by-agent-provider-model");
    assert_eq!(by_apm.len(), 3);
    let planner_apm = by_apm
        .iter()
        .find(|r| {
            r.agent_name.as_deref() == Some("planner")
                && r.provider == "openai"
                && r.model == "gpt-4o"
        })
        .expect("planner/openai/gpt-4o group");
    assert_eq!(planner_apm.request_count, 2);
}

/// W2-12: the runtime-facing projection must use the shared persisted store,
/// keep retries idempotent, scope deltas to a runtime turn, expose uncertainty,
/// and preserve a child agent-run dimension.
#[tokio::test]
async fn code_usage_attribution_runtime_service() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    conn.execute_raw(Statement::from_string(
        conn.get_database_backend(),
        "INSERT INTO config_kv (key, value) VALUES ('libra.repoid', 'repo-canonical-123')"
            .to_string(),
    ))
    .await
    .expect("seed canonical repository id");
    let recorder = UsageRecorder::new(conn.clone());
    let service = RuntimeUsageService::new(recorder.clone());
    let canonical_repo_id = recorder
        .canonical_repo_id()
        .await
        .expect("read canonical repository id");
    assert_eq!(canonical_repo_id.as_deref(), Some("repo-canonical-123"));

    let mut parent = usage_context("unknown-provider", "unknown-model");
    parent.repo_id = canonical_repo_id.clone();
    parent.event_id = Some("parent-event".to_string());
    parent.turn_id = Some("turn-parent".to_string());
    recorder
        .record_summary(
            &parent,
            &CompletionUsageSummary {
                input_tokens: 8,
                output_tokens: 2,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(10),
                cost_usd: None,
            },
            Some(10),
        )
        .await
        .expect("record parent usage");
    // Exact replay is folded by the unique event id.
    recorder
        .record_summary(
            &parent,
            &CompletionUsageSummary {
                input_tokens: 8,
                output_tokens: 2,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(10),
                cost_usd: None,
            },
            Some(10),
        )
        .await
        .expect("replay parent usage");

    let mut child = usage_context_with_agent("openai", "gpt-4o-mini", "reviewer");
    child.repo_id = canonical_repo_id;
    child.agent_run_id = Some("child-run".to_string());
    child.turn_id = parent.turn_id.clone();
    child.event_id = Some("child-event".to_string());
    recorder
        .record_missing_usage(&child, Some(20), 1)
        .await
        .expect("record absent provider usage");
    let repo_ids = conn
        .query_all_raw(Statement::from_string(
            conn.get_database_backend(),
            "SELECT repo_id FROM agent_usage_stats \
             WHERE event_id IN ('parent-event', 'child-event') ORDER BY event_id"
                .to_string(),
        ))
        .await
        .expect("read parent and child repository attribution");
    assert_eq!(repo_ids.len(), 2);
    assert!(
        repo_ids.iter().all(|row| {
            row.try_get_by::<String, _>("repo_id").ok().as_deref() == Some("repo-canonical-123")
        }),
        "parent and child usage rows must share the canonical repo id"
    );

    let mut failed = parent.clone();
    failed.event_id = Some("failed-event".to_string());
    failed.turn_id = Some("turn-failed".to_string());
    recorder
        .record_failure(&failed, "provider_error", Some(30))
        .await
        .expect("record provider failure");

    let parent_turn = service
        .current_turn("turn-parent", UsageQueryFilter::default())
        .await
        .expect("query parent turn");
    assert_eq!(
        parent_turn.request_count, 2,
        "the parent turn must include child-provider requests"
    );
    assert_eq!(parent_turn.total_tokens, 10);
    assert_eq!(
        parent_turn.usage_status,
        UsageStatus::Partial,
        "child usage with an absent provider summary makes the parent turn partial"
    );
    assert_eq!(parent_turn.cost_status, UsageStatus::Unknown);
    assert_eq!(parent_turn.error_status, UsageStatus::Known);

    let child_totals = service
        .sub_agent(
            Some("child-run".to_string()),
            Some("reviewer".to_string()),
            UsageQueryFilter::default(),
        )
        .await
        .expect("query child attribution");
    assert_eq!(child_totals.request_count, 1);
    assert_eq!(child_totals.usage_status, UsageStatus::Unknown);

    let cumulative = service
        .cumulative(UsageQueryFilter::default())
        .await
        .expect("query cumulative totals");
    assert_eq!(cumulative.request_count, 3);
    assert_eq!(cumulative.total_tokens, 10);
    assert_eq!(cumulative.usage_status, UsageStatus::Partial);
    assert_eq!(cumulative.cost_status, UsageStatus::Unknown);
    assert_eq!(cumulative.error_status, UsageStatus::Partial);
    assert_eq!(cumulative.failed_count, 1);

    let mut first_session = usage_context("openai", "gpt-4o-mini");
    first_session.session_id = Some("session-command-a".to_string());
    first_session.turn_id = Some("shared-command-id".to_string());
    first_session.event_id = Some("runtime-turn:shared-command-id".to_string());
    recorder
        .record_summary(
            &first_session,
            &CompletionUsageSummary {
                input_tokens: 3,
                output_tokens: 2,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(5),
                cost_usd: None,
            },
            Some(10),
        )
        .await
        .expect("record first session command");

    let mut second_session = first_session.clone();
    second_session.session_id = Some("session-command-b".to_string());
    recorder
        .record_summary(
            &second_session,
            &CompletionUsageSummary {
                input_tokens: 7,
                output_tokens: 4,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(11),
                cost_usd: None,
            },
            Some(20),
        )
        .await
        .expect("record second session command");

    let first_session_totals = service
        .cumulative(UsageQueryFilter {
            session_id: Some("session-command-a".to_string()),
            ..UsageQueryFilter::default()
        })
        .await
        .expect("query first session command");
    let second_session_totals = service
        .cumulative(UsageQueryFilter {
            session_id: Some("session-command-b".to_string()),
            ..UsageQueryFilter::default()
        })
        .await
        .expect("query second session command");
    assert_eq!(first_session_totals.request_count, 1);
    assert_eq!(first_session_totals.total_tokens, 5);
    assert_eq!(second_session_totals.request_count, 1);
    assert_eq!(second_session_totals.total_tokens, 11);

    let mut failed_only = usage_context("openai", "gpt-4o-mini");
    failed_only.session_id = Some("session-failed-only".to_string());
    failed_only.turn_id = Some("turn-failed-only".to_string());
    failed_only.event_id = Some("runtime-turn:failed-only".to_string());
    recorder
        .record_failure(&failed_only, "provider_error", Some(40))
        .await
        .expect("record all-failed turn");

    let failed_turn = service
        .current_turn("turn-failed-only", UsageQueryFilter::default())
        .await
        .expect("query all-failed turn");
    assert_eq!(failed_turn.request_count, 1);
    assert_eq!(failed_turn.total_tokens, 0);
    assert_eq!(failed_turn.unknown_usage_count, 1);
    assert_eq!(failed_turn.usage_status, UsageStatus::Unknown);
}

#[tokio::test]
async fn runtime_usage_totals_preserve_known_mixed_costs_across_model_groups() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let service = RuntimeUsageService::new(recorder.clone());

    let exact = usage_context("custom", "exact-model");
    recorder
        .record_summary(
            &exact,
            &CompletionUsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(2),
                cost_usd: Some(0.25),
            },
            Some(10),
        )
        .await
        .expect("record exact cost");

    let estimated = usage_context("openai", "gpt-4o-mini");
    recorder
        .record_summary(
            &estimated,
            &CompletionUsageSummary {
                input_tokens: 1_000_000,
                output_tokens: 2_000_000,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(3_000_000),
                cost_usd: None,
            },
            Some(10),
        )
        .await
        .expect("record estimated cost");

    let totals = service
        .cumulative(UsageQueryFilter::default())
        .await
        .expect("query mixed costs");

    assert_eq!(totals.cost_usd, Some(0.25));
    assert_eq!(totals.cost_estimate_micro_dollars, Some(1_350_000));
    assert_eq!(totals.unknown_cost_count, 0);
    assert_eq!(totals.cost_status, UsageStatus::Known);
}

#[tokio::test]
async fn terminal_turn_usage_context_persists_durable_ids_and_current_turn_totals() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let service = RuntimeUsageService::new(recorder.clone());
    let context = usage_context("openai", "gpt-4o-mini").for_runtime_turn("tui-local-123");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 12,
                output_tokens: 8,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(20),
                cost_usd: None,
            },
            Some(50),
        )
        .await
        .expect("record terminal turn usage");

    let row = conn
        .query_one_raw(Statement::from_string(
            conn.get_database_backend(),
            "SELECT turn_id, event_id FROM agent_usage_stats".to_string(),
        ))
        .await
        .expect("query recorded terminal usage")
        .expect("usage row");
    assert_eq!(
        row.try_get_by::<String, _>("turn_id")
            .expect("durable turn id"),
        "tui-local-123"
    );
    assert_eq!(
        row.try_get_by::<String, _>("event_id")
            .expect("durable event id"),
        "runtime-turn:tui-local-123"
    );

    let totals = service
        .current_turn("tui-local-123", UsageQueryFilter::default())
        .await
        .expect("query terminal turn totals");
    assert_eq!(totals.request_count, 1);
    assert_eq!(totals.total_tokens, 20);
}

#[tokio::test]
async fn cancelled_turn_usage_context_persists_durable_ids_and_is_replay_safe() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let service = RuntimeUsageService::new(recorder.clone());
    let context = usage_context("openai", "gpt-4o-mini")
        .for_runtime_turn_cancellation("tui-local-cancelled", 1);

    recorder
        .record_failure(&context, "cancelled_esc", None)
        .await
        .expect("record cancelled turn");
    recorder
        .record_failure(&context, "cancelled_esc", None)
        .await
        .expect("replay cancelled turn");

    let row = conn
        .query_one_raw(Statement::from_string(
            conn.get_database_backend(),
            "SELECT turn_id, event_id, error_kind FROM agent_usage_stats".to_string(),
        ))
        .await
        .expect("query cancelled usage")
        .expect("cancelled usage row");
    assert_eq!(
        row.try_get_by::<String, _>("turn_id")
            .expect("durable turn id"),
        "tui-local-cancelled"
    );
    assert_eq!(
        row.try_get_by::<String, _>("event_id")
            .expect("durable event id"),
        "runtime-turn:tui-local-cancelled:model-turn:1"
    );
    assert_eq!(
        row.try_get_by::<String, _>("error_kind")
            .expect("cancel error kind"),
        "cancelled_esc"
    );

    let totals = service
        .current_turn("tui-local-cancelled", UsageQueryFilter::default())
        .await
        .expect("query cancelled turn totals");
    assert_eq!(totals.request_count, 1, "cancellation replay must dedupe");
    assert_eq!(totals.total_tokens, 0);
    assert_eq!(totals.unknown_usage_count, 1);
}

#[tokio::test]
async fn cancellation_after_model_turn_usage_does_not_double_count_the_turn() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let service = RuntimeUsageService::new(recorder.clone());
    let context = usage_context("openai", "gpt-4o-mini").for_runtime_turn("tui-race-123");
    let mut model_turn_context = context.clone();
    model_turn_context.event_id = context
        .event_id
        .as_ref()
        .map(|event_id| format!("{event_id}:model-turn:1"));

    recorder
        .record_summary(
            &model_turn_context,
            &CompletionUsageSummary {
                input_tokens: 12,
                output_tokens: 8,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(20),
                cost_usd: None,
            },
            Some(50),
        )
        .await
        .expect("record terminal model-turn usage before cancellation");
    recorder
        .record_failure(
            &context.for_runtime_turn_cancellation("tui-race-123", 1),
            "cancelled_esc",
            None,
        )
        .await
        .expect("record cancellation fallback");

    let totals = service
        .current_turn("tui-race-123", UsageQueryFilter::default())
        .await
        .expect("query raced cancellation turn");
    assert_eq!(
        totals.request_count, 1,
        "the cancellation fallback must collide with model-turn:1"
    );
    assert_eq!(totals.total_tokens, 20);
    assert_eq!(totals.failed_count, 0);
    assert_eq!(totals.unknown_usage_count, 0);
}

#[tokio::test]
async fn completed_model_turn_usage_replaces_cancelled_fallback() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-4o-mini").for_runtime_turn("tui-race-cancel-first");
    let cancellation = context.for_runtime_turn_cancellation("tui-race-cancel-first", 1);

    recorder
        .record_failure(&cancellation, "cancelled_esc", None)
        .await
        .expect("record cancellation fallback");
    recorder
        .record_summary(
            &cancellation,
            &CompletionUsageSummary {
                input_tokens: 12,
                output_tokens: 8,
                cached_tokens: Some(3),
                reasoning_tokens: Some(2),
                total_tokens: Some(22),
                cost_usd: Some(0.015),
            },
            Some(50),
        )
        .await
        .expect("record completed model-turn usage after cancellation");

    let row = conn
        .query_one_raw(Statement::from_string(
            conn.get_database_backend(),
            "SELECT prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens, \
             total_tokens, cost_usd, success, error_kind \
             FROM agent_usage_stats \
             WHERE session_id = 'session-1' \
               AND event_id = 'runtime-turn:tui-race-cancel-first:model-turn:1'"
                .to_string(),
        ))
        .await
        .expect("query completed usage")
        .expect("completed usage row");
    assert_eq!(
        row.try_get_by::<i64, _>("prompt_tokens")
            .expect("prompt tokens"),
        12
    );
    assert_eq!(
        row.try_get_by::<i64, _>("completion_tokens")
            .expect("completion tokens"),
        8
    );
    assert_eq!(
        row.try_get_by::<i64, _>("cached_tokens")
            .expect("cached tokens"),
        3
    );
    assert_eq!(
        row.try_get_by::<i64, _>("reasoning_tokens")
            .expect("reasoning tokens"),
        2
    );
    assert_eq!(
        row.try_get_by::<i64, _>("total_tokens")
            .expect("total tokens"),
        22
    );
    assert_eq!(
        row.try_get_by::<f64, _>("cost_usd").expect("exact cost"),
        0.015
    );
    assert_eq!(row.try_get_by::<i64, _>("success").expect("success"), 1);
    assert!(
        row.try_get_by::<Option<String>, _>("error_kind")
            .expect("error kind")
            .is_none(),
        "completed usage must replace cancellation error"
    );

    let totals = RuntimeUsageService::new(recorder)
        .current_turn("tui-race-cancel-first", UsageQueryFilter::default())
        .await
        .expect("query replacement totals");
    assert_eq!(totals.request_count, 1, "the shared event must dedupe");
    assert_eq!(totals.total_tokens, 22);
    assert_eq!(totals.cost_usd, Some(0.015));
    assert_eq!(totals.failed_count, 0);
    assert_eq!(totals.unknown_usage_count, 0);
}

#[tokio::test]
async fn cancellation_during_second_model_turn_records_the_abandoned_request() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let service = RuntimeUsageService::new(recorder.clone());
    let context = usage_context("openai", "gpt-4o-mini").for_runtime_turn("tui-race-two");
    let mut first_model_turn = context.clone();
    first_model_turn.event_id = context
        .event_id
        .as_ref()
        .map(|event_id| format!("{event_id}:model-turn:1"));

    recorder
        .record_summary(
            &first_model_turn,
            &CompletionUsageSummary {
                input_tokens: 12,
                output_tokens: 8,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(20),
                cost_usd: None,
            },
            Some(50),
        )
        .await
        .expect("record completed first model turn");
    recorder
        .record_failure(
            &context.for_runtime_turn_cancellation("tui-race-two", 2),
            "cancelled_esc",
            None,
        )
        .await
        .expect("record cancelled second model turn");

    let totals = service
        .current_turn("tui-race-two", UsageQueryFilter::default())
        .await
        .expect("query cancelled second model turn");
    assert_eq!(totals.request_count, 2);
    assert_eq!(totals.total_tokens, 20);
    assert_eq!(totals.failed_count, 1);
    assert_eq!(totals.unknown_usage_count, 1);

    let events = conn
        .query_all_raw(Statement::from_string(
            conn.get_database_backend(),
            "SELECT event_id FROM agent_usage_stats ORDER BY event_id".to_string(),
        ))
        .await
        .expect("query usage event ids");
    assert_eq!(events.len(), 2);
    assert!(
        events.iter().any(|row| {
            row.try_get_by::<String, _>("event_id").ok().as_deref()
                == Some("runtime-turn:tui-race-two:model-turn:2")
        }),
        "the cancellation fallback must use the in-flight second model-turn key"
    );
}

#[tokio::test]
async fn usage_event_id_without_session_is_rejected() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let mut context = usage_context("openai", "gpt-4o-mini");
    context.session_id = None;
    context.event_id = Some("unscoped-event".to_string());

    let error = recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(2),
                cost_usd: None,
            },
            None,
        )
        .await
        .expect_err("an event ID without a session must be rejected");
    assert!(
        error
            .to_string()
            .contains("usage event_id requires a non-empty session_id"),
        "the recorder must fail closed rather than bypass SQLite's NULL uniqueness semantics"
    );

    let rows = conn
        .query_all_raw(Statement::from_string(
            conn.get_database_backend(),
            "SELECT id FROM agent_usage_stats WHERE event_id = 'unscoped-event'".to_string(),
        ))
        .await
        .expect("query rejected event");
    assert!(rows.is_empty(), "the rejected event must not be persisted");
}
