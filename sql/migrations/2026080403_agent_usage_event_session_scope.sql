-- W2-12 follow-up: browser command IDs are idempotent only within a durable
-- session. A caller can reuse a command ID in another session in the same
-- repository, so the usage-event key must include session identity.
DROP INDEX IF EXISTS `idx_agent_usage_stats_event_id`;
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_usage_stats_event_id`
    ON `agent_usage_stats` (`session_id`, `event_id`);
