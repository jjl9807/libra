-- Reverting to the global event key would reject valid rows written while
-- session-scoped idempotency was enabled. Refuse rather than dropping or
-- rewriting usage attribution during a rollback.
CREATE TABLE IF NOT EXISTS `_agent_usage_event_session_scope_down_guard` (`probe` INTEGER);
CREATE TRIGGER `_agent_usage_event_session_scope_down_guard_duplicates`
BEFORE INSERT ON `_agent_usage_event_session_scope_down_guard`
WHEN EXISTS (
    SELECT 1
    FROM `agent_usage_stats`
    WHERE `event_id` IS NOT NULL
    GROUP BY `event_id`
    HAVING COUNT(*) > 1
)
BEGIN
    SELECT RAISE(ABORT, 'cannot roll back session-scoped usage event IDs while duplicate event_id values exist; keep migration 2026080403 applied or rekey duplicate event_id values before retrying');
END;
INSERT INTO `_agent_usage_event_session_scope_down_guard` (`probe`) VALUES (1);
DROP TRIGGER `_agent_usage_event_session_scope_down_guard_duplicates`;
DROP TABLE `_agent_usage_event_session_scope_down_guard`;

DROP INDEX IF EXISTS `idx_agent_usage_stats_event_id`;
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_usage_stats_event_id`
    ON `agent_usage_stats` (`event_id`);
