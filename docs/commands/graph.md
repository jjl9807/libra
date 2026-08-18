# `libra graph`

Inspect a Libra Code thread version graph.

## Synopsis

```bash
libra --json graph <THREAD_ID> [--repo <PATH>]
libra --machine graph <THREAD_ID> [--repo <PATH>]
```

## Description

The current thread/version graph is shown in Web Code UI (`libra code`). `libra graph` still loads the same projection tables and formal AI history under `.libra/`.

**Breaking change (W5-08):** the interactive graph TUI entry was removed in the W5 breaking release. Bare `libra graph <THREAD_ID>` now fails with a usage error plus a migration hint; open the live graph in Web Code UI (`libra code`) instead, or use `libra graph --json` / `--machine` for the structured output, which is unchanged.

The graph is a version chain of:

- Intent revisions
- Execution plans
- Tasks
- Runs
- PatchSets

The graph highlights current/latest intent heads, selected plan heads, active tasks/runs, latest runs, and latest patchsets when that projection data is available.

With the global `--json` (or `--machine`) flag, `libra graph` emits the graph as a structured JSON document — the agent-friendly path. The envelope's `data` carries the thread metadata (`thread_id`, `title`, `freshness`, `thread_version`, `scheduler_version`, `updated_at`, and the `selected_plan_id` / `active_task_id` / `active_run_id` heads) plus optional Code UI overlays folded from the newest matching session workflow log:

| Field | Type | Meaning |
|-------|------|---------|
| `code_ui_status` | string or null | Wire `snake_case` Code UI status (`idle`, `thinking`, `executing_tool`, `awaiting_interaction`, `completed`, `error`, `indeterminate_side_effect`). Null when no session is attached to the thread. |
| `code_ui_transcript_len` | number | Folded transcript entry count (0 when no overlay is available). |
| `code_ui_pending_interactions` | number | Count of pending Code UI interactions after the fold (0 when no overlay is available). |

A missing session is non-fatal (overlay fields stay null/zero). An unreadable or malformed session workflow log is a hard error so an indeterminate reconciliation fence cannot be hidden. On first overlay lookup, Libra may run a one-time thread→session index rebuild under `.libra/.../sessions/.thread_index/` (also performed when `libra code` starts); that migration fails closed if any existing session cannot be loaded or indexed. Pending mutating commands without a live owner surface as `indeterminate_side_effect` even before a runtime restart fences them. The payload also includes a `nodes` array; each node has its `depth`, `kind` (`intent` / `plan` / `task` / `run` / `patchset`), `id`, `label`, `tags`, key/value `detail`, and — when the underlying AI object was loaded — an `object` with its `object_type`, `hash`, `git_object_type`, and a `summary` of key fields.

Example — inspect a thread's version graph as structured JSON (the compact form uses `--machine`):

```bash
libra graph --json 11111111-1111-4111-8111-111111111111
```

The bare interactive entry was removed in the W5 breaking release (W5-08) and fails with a usage error plus a migration hint; the legacy `libra code` TUI session that printed this follow-up command on exit was removed together with the TUI startup path (W5-06). For an interactive view, open the thread's graph in Web Code UI instead.

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<THREAD_ID>` | yes | Canonical Libra thread UUID to inspect. |

## Options

| Option | Description |
|--------|-------------|
| `--repo <PATH>` | Inspect a specific Libra repository instead of discovering one from the current directory. |

## Common Commands

```bash
# Emit the version graph for a thread as structured JSON
libra graph --json 11111111-1111-4111-8111-111111111111

# Emit the graph for a thread in another working tree
libra graph --json 11111111-1111-4111-8111-111111111111 --repo /path/to/repo

# Resume the same thread in Code after inspection
libra code --resume 11111111-1111-4111-8111-111111111111
```

## Output

`libra graph` requires the global `--json` (or `--machine`) flag: without it the command exits with a usage error and a migration hint (the interactive TUI entry was removed in the W5 breaking release; the live graph view is in Web Code UI). With `--json`/`--machine` it writes the graph as a single structured JSON document to stdout (the envelope described under *Description*), so it can run headless in agent/automation contexts. The command exits with a usage error if the thread ID is not a UUID, and with a repository/projection error if neither the current directory nor `--repo` resolves to a Libra repository, or if the requested thread cannot be found.

## Design Notes

The graph uses Libra's projection read model instead of parsing TUI session JSON directly. That keeps the view provider-neutral: generic LLM sessions and managed Codex sessions can both be inspected as long as they have formal AI history.

The command accepts the canonical Libra thread ID, not a provider-specific session ID. `libra code` prints the canonical command after exit when it can derive one from session metadata, the Code UI projection, or the latest formal AI history in the repository.
