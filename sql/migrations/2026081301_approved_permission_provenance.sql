-- plan-20260715 W4-07: approved_permission provenance + Repository ownership.
--
-- Adds audit-only provenance columns for Always approvals so a linked worktree
-- or session that recorded the approval is recoverable without changing the
-- matching key. Existing rows keep empty provenance ('').
--
-- project_id is NOT rewritten here: opaque legacy values stay until an
-- explicit doctor adopt/cleanup (no silent merge onto libra.repoid).

ALTER TABLE `approved_permission` ADD COLUMN `source_worktree_id` TEXT NOT NULL DEFAULT '';
ALTER TABLE `approved_permission` ADD COLUMN `source_session_id` TEXT NOT NULL DEFAULT '';
ALTER TABLE `approved_permission` ADD COLUMN `source_workspace_id` TEXT NOT NULL DEFAULT '';
