//! Read-only checkpoint-input materialization (plan-20260714 PD-02).
//!
//! A checkpoint-scoped `libra review --checkpoint <id>` /
//! `libra investigate --checkpoint <id>` run does NOT review the working
//! tree: the reviewers'/investigators' whole workspace is the checkpoint's
//! captured content — metadata, manifest, transcript parts — materialized
//! as READ-ONLY files inside the run directory
//! (`<run_dir>/checkpoint-input/`). This is deliberately not disguised as
//! a worktree diff: the materialized tree mirrors the checkpoint's inner
//! tree byte-for-byte, and the scoped prompt tells the agent it is
//! looking at a captured transcript, not a repository snapshot.
//!
//! Lifecycle / retention: the materialization lives inside the run
//! directory, so it shares the run's lifecycle exactly — `review clean` /
//! `investigate clean` remove it with the run, the orphaned-run cancel
//! path releases it through the recorded `workspace_root`, and
//! `agent doctor` needs no new orphan class (there is no storage outside
//! the run directory; the durable source of truth remains the checkpoint
//! objects themselves).
//!
//! The spec is produced by the command layer (which owns checkpoint
//! layout knowledge and fails closed BEFORE any run exists when the
//! checkpoint is missing, malformed, or not locally materializable);
//! this module only turns an already-validated spec into files.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use git_internal::hash::ObjectHash;
use serde::{Deserialize, Serialize};

use crate::utils::object::read_git_object_bounded;

/// Directory name of the materialized input inside the run directory.
pub const CHECKPOINT_INPUT_DIR: &str = "checkpoint-input";

/// Per-file byte cap. A transcript part larger than this fails the
/// materialization closed (corrupt or hostile checkpoint) rather than
/// filling the disk.
pub const CHECKPOINT_INPUT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Total materialized-bytes cap across every file of one checkpoint.
pub const CHECKPOINT_INPUT_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// One file of the checkpoint's inner tree, identified by its blob oid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointInputFile {
    /// Path relative to the checkpoint's inner tree root (`metadata.json`,
    /// `transcript/claude_code`, …), using `/` separators.
    pub rel_path: String,
    pub oid: String,
}

/// Validated materialization plan for one checkpoint — every listed blob
/// was confirmed locally present by the resolver before any run side
/// effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointInputSpec {
    pub checkpoint_id: String,
    pub files: Vec<CheckpointInputFile>,
}

/// Materialize `spec` under `<run_dir>/checkpoint-input/`, returning the
/// materialized root. Files are written read-only (0444 on Unix); any
/// failure returns a redacted, human-readable reason (the caller records
/// it as the run's `infra_error`).
pub fn materialize_checkpoint_input(
    storage: &Path,
    spec: &CheckpointInputSpec,
    run_dir: &Path,
) -> Result<PathBuf, String> {
    let root = run_dir.join(CHECKPOINT_INPUT_DIR);
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("failed to create checkpoint input dir: {e}"))?;
    let mut total: u64 = 0;
    for file in &spec.files {
        let rel = sanitize_rel_path(&file.rel_path)?;
        let oid = ObjectHash::from_str(&file.oid).map_err(|e| {
            format!(
                "invalid blob oid '{}' in checkpoint input spec: {e}",
                file.oid
            )
        })?;
        let (bytes, truncated) =
            read_git_object_bounded(storage, &oid, CHECKPOINT_INPUT_MAX_FILE_BYTES).map_err(
                |e| {
                    format!(
                        "checkpoint blob {} ({}) is not readable from the local object store: {e}",
                        file.oid, file.rel_path
                    )
                },
            )?;
        if truncated {
            return Err(format!(
                "checkpoint blob {} ({}) exceeds the {CHECKPOINT_INPUT_MAX_FILE_BYTES}-byte \
                 per-file cap; refusing to materialize",
                file.oid, file.rel_path
            ));
        }
        total = total.saturating_add(bytes.len() as u64);
        if total > CHECKPOINT_INPUT_MAX_TOTAL_BYTES {
            return Err(format!(
                "checkpoint {} materialization exceeds the \
                 {CHECKPOINT_INPUT_MAX_TOTAL_BYTES}-byte total cap; refusing",
                spec.checkpoint_id
            ));
        }
        let dest = root.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create checkpoint input subdir: {e}"))?;
        }
        std::fs::write(&dest, &bytes).map_err(|e| {
            format!(
                "failed to write checkpoint input file {}: {e}",
                file.rel_path
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o444));
        }
    }
    Ok(root)
}

/// Reject absolute/parent-escaping components: the spec's rel paths come
/// from a checkpoint tree, but the materializer re-validates so a corrupt
/// tree can never write outside the input dir.
fn sanitize_rel_path(rel: &str) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for component in rel.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!(
                "checkpoint input path '{rel}' contains an unsafe component; refusing"
            ));
        }
        out.push(component);
    }
    if out.as_os_str().is_empty() {
        return Err("checkpoint input path is empty; refusing".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_escapes() {
        for bad in ["../x", "a/../b", "/abs", "a//b", "", "."] {
            assert!(sanitize_rel_path(bad).is_err(), "{bad} must be rejected");
        }
        assert_eq!(
            sanitize_rel_path("transcript/claude_code").unwrap(),
            PathBuf::from("transcript/claude_code")
        );
    }
}
