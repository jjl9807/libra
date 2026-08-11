-- plan-20260715 W2-12: durable repository/turn/event attribution for the
-- shared runtime usage read model. event_id is nullable for legacy callers;
-- SQLite UNIQUE permits multiple NULLs while deduplicating runtime events.
ALTER TABLE `agent_usage_stats` ADD COLUMN `repo_id` TEXT;
ALTER TABLE `agent_usage_stats` ADD COLUMN `turn_id` TEXT;
ALTER TABLE `agent_usage_stats` ADD COLUMN `event_id` TEXT;
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_repo_session`
    ON `agent_usage_stats` (`repo_id`, `session_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_turn`
    ON `agent_usage_stats` (`turn_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_usage_stats_agent_run`
    ON `agent_usage_stats` (`agent_run_id`);
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_usage_stats_event_id`
    ON `agent_usage_stats` (`event_id`);
