-- Rollback of 2026081301_approved_permission_provenance.
--
-- REFUSES while any row carries non-empty provenance, or while this repository
-- shows linked-worktree evidence: dropping the columns would erase the only
-- audit trail for which worktree/session recorded an Always approval, and a
-- linked repository is exactly where that trail matters.
--
-- Clear provenance (or delete those rows) and ensure no linked HEAD refs
-- remain before retrying the down migration.

CREATE TABLE IF NOT EXISTS `approved_permission_provenance_down_guard` (
    `blocked` INTEGER NOT NULL CHECK (`blocked` = 0)
);

INSERT INTO `approved_permission_provenance_down_guard` (`blocked`)
SELECT
    (
        SELECT COUNT(*) FROM `approved_permission`
        WHERE `source_worktree_id` != ''
           OR `source_session_id` != ''
           OR `source_workspace_id` != ''
    )
    +
    (
        SELECT COUNT(*) FROM `reference`
        WHERE `kind` = 'Head' AND `remote` IS NULL AND `worktree_id` IS NOT NULL
    );

DROP TABLE `approved_permission_provenance_down_guard`;

ALTER TABLE `approved_permission` DROP COLUMN `source_workspace_id`;
ALTER TABLE `approved_permission` DROP COLUMN `source_session_id`;
ALTER TABLE `approved_permission` DROP COLUMN `source_worktree_id`;
