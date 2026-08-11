-- `repo_id`, `turn_id`, and `event_id` are durable attribution and
-- idempotency identities. The pre-2026080402 schema cannot represent them,
-- so fail closed instead of silently discarding populated identities.
CREATE TABLE IF NOT EXISTS `_agent_usage_runtime_attribution_down_guard` (`probe` INTEGER);
CREATE TRIGGER `_agent_usage_runtime_attribution_down_guard_populated_dimensions`
BEFORE INSERT ON `_agent_usage_runtime_attribution_down_guard`
WHEN EXISTS (
    SELECT 1
    FROM `agent_usage_stats`
    WHERE `repo_id` IS NOT NULL
       OR `turn_id` IS NOT NULL
       OR `event_id` IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back runtime usage attribution while repo_id, turn_id, or event_id values exist; keep migration 2026080402 applied or remove the attributed usage rows before retrying');
END;
INSERT INTO `_agent_usage_runtime_attribution_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_usage_runtime_attribution_down_guard_populated_dimensions`;
DROP TABLE `_agent_usage_runtime_attribution_down_guard`;

DROP INDEX IF EXISTS `idx_agent_usage_stats_event_id`;
DROP INDEX IF EXISTS `idx_agent_usage_stats_agent_run`;
DROP INDEX IF EXISTS `idx_agent_usage_stats_turn`;
DROP INDEX IF EXISTS `idx_agent_usage_stats_repo_session`;
CREATE TABLE IF NOT EXISTS `agent_usage_stats__rebuild` (
    `id` TEXT PRIMARY KEY, `session_id` TEXT, `thread_id` TEXT,
    `agent_run_id` TEXT, `run_id` TEXT, `provider` TEXT NOT NULL,
    `model` TEXT NOT NULL, `agent_name` TEXT,
    `request_kind` TEXT NOT NULL DEFAULT 'completion', `intent` TEXT,
    `prompt_tokens` INTEGER NOT NULL DEFAULT 0,
    `completion_tokens` INTEGER NOT NULL DEFAULT 0,
    `cached_tokens` INTEGER NOT NULL DEFAULT 0,
    `reasoning_tokens` INTEGER NOT NULL DEFAULT 0,
    `total_tokens` INTEGER NOT NULL DEFAULT 0,
    `tool_call_count` INTEGER NOT NULL DEFAULT 0,
    `wall_clock_ms` INTEGER NOT NULL DEFAULT 0, `provider_latency_ms` INTEGER,
    `cost_estimate_micro_dollars` INTEGER, `cost_usd` REAL,
    `usage_estimated` INTEGER NOT NULL DEFAULT 0, `started_at` TEXT,
    `finished_at` TEXT, `success` INTEGER NOT NULL DEFAULT 1,
    `error_kind` TEXT, `schema_version` INTEGER NOT NULL DEFAULT 1,
    `created_at` TEXT NOT NULL
);
INSERT INTO `agent_usage_stats__rebuild`
SELECT id, session_id, thread_id, agent_run_id, run_id, provider, model,
       agent_name, request_kind, intent, prompt_tokens, completion_tokens,
       cached_tokens, reasoning_tokens, total_tokens, tool_call_count,
       wall_clock_ms, provider_latency_ms, cost_estimate_micro_dollars,
       cost_usd, usage_estimated, started_at, finished_at, success,
       error_kind, schema_version, created_at FROM `agent_usage_stats`;
DROP TABLE `agent_usage_stats`;
ALTER TABLE `agent_usage_stats__rebuild` RENAME TO `agent_usage_stats`;
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_provider_model`
    ON `agent_usage_stats` (`provider`, `model`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_thread`
    ON `agent_usage_stats` (`thread_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_session`
    ON `agent_usage_stats` (`session_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_started`
    ON `agent_usage_stats` (`started_at`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_agent_name_provider_model`
    ON `agent_usage_stats` (`agent_name`, `provider`, `model`);
