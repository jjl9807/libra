//! SeaORM entity definition for command-level operation audit records.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "operation")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub op_id: String,
    pub repo_id: String,
    pub view_id: String,
    pub command_name: String,
    pub description: String,
    pub actor: String,
    pub args_digest: Option<String>,
    pub start_ts: i64,
    pub end_ts: Option<i64>,
    pub status: String,
    /// Worktree scope the operation ran in (Part C W1 §C.9): main = `""`,
    /// linked = its stable instance id. Scopes the duplicate-submission
    /// window per-worktree.
    pub worktree_id: String,
    /// How `worktree_id` came to hold its value (Part C W0 §C.11):
    /// `"declared"` — the process that ran the operation recorded its own
    /// scope; `"unknown"` — the row predates the scope column in a
    /// repository with linked-worktree evidence, so its `""` means "not
    /// recorded", not "main". `op restore` refuses `unknown` rows rather
    /// than guess (ADR-0714-08).
    pub scope_provenance: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
