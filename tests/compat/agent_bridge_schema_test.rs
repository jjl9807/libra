//! `tests/compat/agent_bridge_schema_test.rs` — guard pinning the bridge
//! durable-projection migration (plan-20260818 LB-02) to the registered
//! migration set and its SQL files.
//!
//! A silent registry regression (forgetting to register the migration, or
//! bumping the version pin without adding a migration) surfaces here as well
//! as in `db_migration_test`/`agent_bridge_migration_test`.

use libra::internal::db::migration::builtin_runner;

const BRIDGE_MIGRATION_VERSION: i64 = 2026081801;

#[test]
fn bridge_migration_is_registered_and_is_the_latest() {
    let runner = builtin_runner().expect("builtin registry builds clean");
    assert_eq!(
        runner.max_registered_version(),
        Some(BRIDGE_MIGRATION_VERSION),
        "2026081801_agent_bridge_capture must be the latest registered migration"
    );
}

#[test]
fn bridge_migration_sql_defines_all_bridge_tables() {
    let up = include_str!("../../sql/migrations/2026081801_agent_bridge_capture.sql");
    for table in [
        "agent_bridge_session",
        "agent_bridge_event",
        "agent_bridge_operation",
        "agent_bridge_checkpoint",
        "agent_bridge_link",
    ] {
        assert!(
            up.contains(&format!("CREATE TABLE IF NOT EXISTS `{table}`")),
            "{table} must be created by the migration SQL"
        );
    }
    // The fixed source constraint must be present (ADR-LB-02).
    assert!(
        up.contains("deepseek-harness"),
        "source must be fixed to deepseek-harness"
    );
}

#[test]
fn bridge_migration_down_is_forward_only_freeze() {
    let down = include_str!("../../sql/migrations/2026081801_agent_bridge_capture_down.sql");
    // The down path must guard against deleting bridge rows, never silently
    // drop acked data as a rollback shortcut (GC-LB-09 / ER-LB-04).
    assert!(
        down.contains("_down_guard") && down.contains("CHECK"),
        "down must freeze (refuse) while bridge rows exist"
    );
}
