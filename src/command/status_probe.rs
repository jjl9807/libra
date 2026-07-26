//! Bounded rename-destination probe (plan-20260714 R0-3, §B.3.1–§B.3.2,
//! §B.3.5).
//!
//! Under the `status.renameUntracked` extension, unstaged rename
//! DESTINATIONS come from this probe — decoupled from the untracked display
//! scan (`-uno` hides display markers but never the probe), bounded by a
//! call-global dual budget, and qualified by the same tracked/ignore
//! layering the display scan uses. The probe never injects `?`/`??` markers
//! into the display layer.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use git_internal::internal::index::Index;

use crate::{
    command::status_untracked_paths::TrackedPaths,
    utils::{pathspec::PathspecSet, util},
};

/// Cross-root enumeration budget (§B.3.2): every entry taken from a
/// `read_dir` counts, including ones excluded afterwards.
pub(crate) const PROBE_MAX_ENUMERATED_ENTRIES: usize = 50_000;
/// Qualified-destination budget (§B.3.2): counted separately so candidate
/// storage stays bounded even under the enumeration cap.
pub(crate) const PROBE_MAX_QUALIFIED_DESTINATIONS: usize = 10_000;

/// Injectable probe limits (GC-DR-07 pattern: debug-only env overrides so
/// tests can trip the budgets without 50k-file fixtures).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProbeLimits {
    pub(crate) max_enumerated_entries: usize,
    pub(crate) max_qualified_destinations: usize,
}

impl ProbeLimits {
    pub(crate) fn effective() -> Self {
        let read = |name: &str, default: usize| -> usize {
            if cfg!(debug_assertions)
                && let Ok(value) = std::env::var(name)
                && let Ok(parsed) = value.parse::<usize>()
                && parsed > 0
            {
                return parsed.min(default);
            }
            default
        };
        Self {
            max_enumerated_entries: read(
                "LIBRA_TEST_STATUS_PROBE_ENUM_BUDGET",
                PROBE_MAX_ENUMERATED_ENTRIES,
            ),
            max_qualified_destinations: read(
                "LIBRA_TEST_STATUS_PROBE_DEST_BUDGET",
                PROBE_MAX_QUALIFIED_DESTINATIONS,
            ),
        }
    }
}

/// Why a path could not be inspected (§B.6.0.1 reason taxonomy; the JSON
/// serde contract lands with R0-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoBlockedReason {
    PermissionDenied,
    IoError,
}

impl IoBlockedReason {
    fn from_error(error: &io::Error) -> Self {
        if error.kind() == io::ErrorKind::PermissionDenied {
            IoBlockedReason::PermissionDenied
        } else {
            IoBlockedReason::IoError
        }
    }
}

/// One blocked path (workdir-relative) with its reason.
#[derive(Debug, Clone)]
pub(crate) struct IoBlockedEvent {
    pub(crate) path: PathBuf,
    /// Reason taxonomy (§B.6.0.1), consumed by the io_blocked[] JSON
    /// contract (R0-8) and its worktree-family warning mapping.
    pub(crate) reason: IoBlockedReason,
}

/// Which budget tripped first when `truncated` is set (JSON warning must
/// attribute the cause, §B.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeBudgetKind {
    Enumeration,
    Destination,
}

/// Composite probe outcome across every root (§B.3.2 归并).
#[derive(Debug, Default)]
pub(crate) struct ProbeOutcome {
    /// Qualified destinations (workdir-relative), stably sorted.
    pub(crate) destinations: Vec<PathBuf>,
    /// A budget tripped; partial destinations are still pairable.
    pub(crate) truncated: Option<ProbeBudgetKind>,
    /// Every I/O failure encountered (path-sorted, deduplicated).
    pub(crate) io_blocked: Vec<IoBlockedEvent>,
    /// Candidates skipped because their name is not valid UTF-8 (R0 scope:
    /// they keep their base `??` behavior but never join rename scoring —
    /// surfaced as one `rename_path_encoding_unsupported` warning).
    pub(crate) encoding_skipped: u64,
}

/// Derive the probe roots from the positive pathspecs (§B.3.1.1) —
/// workdir-relative, `""` meaning the repository root. Case-folded or
/// otherwise unnarrowable specs conservatively fall back to the root;
/// redundant nested roots are removed.
pub(crate) fn pathspec_probe_roots(pathspecs: Option<&PathspecSet>) -> Vec<PathBuf> {
    let Some(set) = pathspecs else {
        return vec![PathBuf::new()];
    };
    if set.is_empty() || !set.has_positive() {
        // No specs, or exclude-only (positive set ≡ whole repository).
        return vec![PathBuf::new()];
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in set.positive_depth_roots() {
        if root.icase() {
            // Cannot narrow safely on a case-sensitive FS: probe everything.
            return vec![PathBuf::new()];
        }
        roots.push(root.path().to_path_buf());
    }
    roots.sort();
    roots.dedup();
    if roots.iter().any(|r| r.as_os_str().is_empty()) {
        return vec![PathBuf::new()];
    }
    // Drop roots covered by an ancestor root (`a/` covers `a/b/`).
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !kept.iter().any(|prev| root.starts_with(prev)) {
            kept.push(root);
        }
    }
    kept
}

/// Inputs for destination qualification (§B.3.1.2): the same tracked/ignore
/// layering as the display scan.
pub(crate) struct DestinationFilter<'a> {
    pub(crate) workdir: &'a Path,
    pub(crate) index: &'a Index,
    pub(crate) tracked: &'a TrackedPaths,
    pub(crate) pathspecs: Option<&'a PathspecSet>,
}

impl DestinationFilter<'_> {
    /// §B.3.1.2: keep a probed path only when it matches the pathspec set,
    /// is not tracked (including a case-fold alias), not unmerged, and not
    /// ignored.
    fn qualifies(&self, relative: &Path, absolute: &Path, encoding_skipped: &mut u64) -> bool {
        if let Some(set) = self.pathspecs
            && !set.matches_path(relative)
        {
            return false;
        }
        let Some(rel_str) = relative.to_str() else {
            // Non-UTF-8 paths stay out of rename candidacy in R0 (DEFER-02);
            // base D/A/`??` behavior is unaffected (§B.6.1) and the skip is
            // counted for one deduplicated
            // `rename_path_encoding_unsupported` warning.
            *encoding_skipped += 1;
            return false;
        };
        if self.index.tracked(rel_str, 0)
            || (1..=3).any(|stage| self.index.get(rel_str, stage).is_some())
        {
            return false;
        }
        if self.tracked.same_file_case_alias(self.workdir, relative) {
            return false;
        }
        if util::check_gitignore(self.workdir, absolute) {
            return false;
        }
        true
    }
}

/// Walk the probe roots with the shared dual budget (§B.3.2). Never follows
/// symlinks (they are leaf candidates), never enters `.libra`/`.git` or a
/// nested repository, records EACCES/I-O as events while continuing
/// siblings, and treats a `NotFound` entry as a TOCTOU no-op.
pub(crate) fn probe_rename_destinations(
    roots: &[PathBuf],
    filter: &DestinationFilter<'_>,
    limits: ProbeLimits,
) -> ProbeOutcome {
    let mut outcome = ProbeOutcome::default();
    let mut enumerated = 0usize;
    let workdir = filter.workdir;

    'roots: for root in roots {
        let root_abs = workdir.join(root);
        match std::fs::symlink_metadata(&root_abs) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue, // gone
            Err(error) => {
                outcome.io_blocked.push(IoBlockedEvent {
                    path: root.clone(),
                    reason: IoBlockedReason::from_error(&error),
                });
                continue;
            }
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                // A file/symlink root is itself a candidate.
                enumerated += 1;
                if enumerated > limits.max_enumerated_entries {
                    outcome.truncated = Some(ProbeBudgetKind::Enumeration);
                    break 'roots;
                }
                if filter.qualifies(root, &root_abs, &mut outcome.encoding_skipped) {
                    if outcome.destinations.len() >= limits.max_qualified_destinations {
                        outcome.truncated = Some(ProbeBudgetKind::Destination);
                        break 'roots;
                    }
                    outcome.destinations.push(root.clone());
                }
                continue;
            }
            Ok(_) => {}
        }

        let mut pending: Vec<PathBuf> = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            let dir_abs = workdir.join(&dir);
            let reader = match std::fs::read_dir(&dir_abs) {
                Ok(reader) => reader,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    outcome.io_blocked.push(IoBlockedEvent {
                        path: dir.clone(),
                        reason: IoBlockedReason::from_error(&error),
                    });
                    continue;
                }
            };
            // Bounded enumeration: charge the budget per taken entry and
            // stop immediately on the trip — never keep reading for sort
            // stability (§B.3.2 determinism carve-out for truncation).
            let mut names: Vec<PathBuf> = Vec::new();
            for entry in reader {
                enumerated += 1;
                if enumerated > limits.max_enumerated_entries {
                    outcome.truncated = Some(ProbeBudgetKind::Enumeration);
                    break;
                }
                match entry {
                    Ok(entry) => names.push(dir.join(entry.file_name())),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        outcome.io_blocked.push(IoBlockedEvent {
                            path: dir.clone(),
                            reason: IoBlockedReason::from_error(&error),
                        });
                        continue;
                    }
                }
            }
            // Fully-enumerated directories process in byte order for
            // deterministic recursion/qualification.
            names.sort();
            for relative in names {
                let absolute = workdir.join(&relative);
                let name = relative.file_name().unwrap_or_default();
                if name == std::ffi::OsStr::new(util::ROOT_DIR)
                    || name == std::ffi::OsStr::new(util::GIT_DIR)
                {
                    continue;
                }
                let meta = match std::fs::symlink_metadata(&absolute) {
                    Ok(meta) => meta,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        outcome.io_blocked.push(IoBlockedEvent {
                            path: relative.clone(),
                            reason: IoBlockedReason::from_error(&error),
                        });
                        continue;
                    }
                };
                if meta.file_type().is_symlink() || meta.is_file() {
                    if filter.qualifies(&relative, &absolute, &mut outcome.encoding_skipped) {
                        if outcome.destinations.len() >= limits.max_qualified_destinations {
                            outcome.truncated = Some(ProbeBudgetKind::Destination);
                            break;
                        }
                        outcome.destinations.push(relative);
                    }
                } else if meta.is_dir() {
                    // Prune ignored subtrees before enumerating them and
                    // never enter a nested repository or gitlink checkout.
                    if util::check_gitignore(workdir, &absolute)
                        || absolute.join(util::ROOT_DIR).exists()
                        || absolute.join(util::GIT_DIR).exists()
                    {
                        continue;
                    }
                    pending.push(relative);
                }
            }
            if outcome.truncated.is_some() {
                break 'roots;
            }
        }
    }

    outcome.destinations.sort();
    outcome.destinations.dedup();
    outcome.io_blocked.sort_by(|a, b| a.path.cmp(&b.path));
    outcome.io_blocked.dedup_by(|a, b| a.path == b.path);
    outcome
}

/// §B.3.5: collapse `? dir/` markers consumed by rename pairing (display
/// base). A marker is removed only when the probe was COMPLETE (no
/// truncation, no I/O blocks) and no qualified-but-unconsumed candidate
/// remains under the directory; incomplete probes conservatively keep every
/// marker.
pub(crate) fn collapse_untracked_markers(
    unstaged_new: &mut Vec<PathBuf>,
    destinations: &[PathBuf],
    consumed: &HashSet<PathBuf>,
    probe_complete: bool,
) {
    if !probe_complete {
        return;
    }
    unstaged_new.retain(|entry| {
        let text = entry.to_string_lossy();
        if !text.ends_with('/') {
            // Plain untracked row consumed as a destination: drop it (the
            // rename record is its signal now).
            return !consumed.contains(entry);
        }
        let dir = PathBuf::from(text.trim_end_matches('/'));
        let mut saw_candidate = false;
        for destination in destinations {
            if destination.starts_with(&dir) {
                saw_candidate = true;
                if !consumed.contains(destination) {
                    return true; // unconsumed candidate remains → keep marker
                }
            }
        }
        // Every candidate under the dir was consumed → remove the marker; a
        // dir the probe never saw keeps its marker conservatively.
        !saw_candidate
    });
}
