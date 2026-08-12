# `libra code`

Launch an interactive AI coding session with TUI, web, or MCP modes.

## Synopsis

```
libra code
libra code --web-only [-p <PORT>] [--host <HOST>]
libra code --stdio
libra code --provider <PROVIDER> [--model <MODEL>]
libra code --resume <THREAD_ID>
libra graph <THREAD_ID> [--repo <PATH>]
```

## Description

`libra code` starts an interactive coding session that pairs a human developer with an AI agent. The default mode launches the Web Code UI (embedded HTTP server + AgentRuntime) and prints the URL / control details; it stays in the foreground until `Ctrl-C` / SIGTERM. `--web` / `--web-only` remain accepted as deprecated no-op aliases for that default during the W4 bake window (removed in W5-07). Hidden `LIBRA_CODE_LEGACY_TUI=1` restores the previous terminal UI for emergency rollback only (not a public compatibility surface). `--stdio` is a **deprecated MCP-only legacy** entry: it exposes MCP tools/resources over standard input/output for clients like Claude Desktop, and is **not** live turn control. Prefer `libra code --control stdio` for local automation; a dedicated `libra mcp --stdio` is planned after W5 (DEFER-02).

The command supports eight AI provider backends (Gemini, OpenAI, Anthropic, DeepSeek, Kimi, Zhipu, Ollama, Codex) and three operating contexts (dev, review, research) that tune the agent's behavior for different workflows. Sessions can be persisted and resumed with Libra's canonical `--resume <thread_id>` flow. Passing `--goal "<objective>"` boots the session directly in goal mode, where a supervisor drives the tool loop toward the stated objective until a verifier accepts completion.

A sandboxed tool-execution layer enforces approval policies that control when the agent can run shell commands, apply patches, web search, or perform other potentially destructive operations. Legacy TUI (`LIBRA_CODE_LEGACY_TUI=1`) and headless Web sessions in the `dev` context default to workspace-write execution with network access denied. After the execution plan is ready, the Plan review dialog offers Execute Plan / Modify Plan / Cancel. Choosing Execute opens a separate mandatory network-policy prompt (`Network: Deny` / `Network: Allow` / `Back`); the choice is applied only after that gate resolves, and both gates are durable across crash/resume. Review and research contexts remain read-only and do not grant network access.

When the legacy TUI exits and Libra can derive the canonical thread ID, `libra code` prints a follow-up `libra graph <thread_id>` command so the thread's Intent/Plan/Task/Run/PatchSet version graph can be inspected in a separate TUI. Use `libra graph <thread_id> --repo <path>` when inspecting a repository other than the current directory.

**Linked worktrees**: `libra code` (every mode) refuses to launch from a linked worktree until the unified Code/Agent configuration resolver lands — the session's configuration (agents, hooks, sandbox, approvals) is not yet worktree-aware there, and a linked session could run under the repository's sandbox policy without its security hooks. Run from the main worktree instead; the refusal names this remedy. For the same reason, automation VCS dispatch is disabled (with a warning) in linked worktrees and `libra automation` fails closed there — see [automation.md](automation.md).

## Options

| Flag | Short | Long | Default | Description |
|------|-------|------|---------|-------------|
| Web only | | `--web-only` / `--web` | off | Deprecated alias for the default Web Code UI (W4 bake window; removed in W5-07). Conflicts with `--stdio`. |
| Port | `-p` | `--port` | `3000` | Web server listen port. |
| Host | | `--host` | `127.0.0.1` | Web server bind address. |
| Working directory | | `--cwd` | current dir | Working directory for the session. |
| Env file | | `--env-file <PATH>` | none | Load provider environment variables from a dotenv-style file; explicit file values take precedence over Vault and the process environment. |
| Control mode | | `--control <observe\|write\|stdio>` | `observe` | Local automation control mode. `observe` preserves existing loopback read behavior; `write` enables local token discovery and process-level automation control auth; `stdio` is a **client-only** JSON-RPC NDJSON shim that drives an existing write-control session (no Web/TUI/MCP launch). |
| Control token file | | `--control-token-file <PATH>` | `.libra/code/control-token` | Path for the per-process local automation token. In `write` mode, Unix/macOS files must be regular files with `0600` permissions. With `--control stdio`, overrides the worktree default token path (still independent of `--control-info-file`); overly permissive modes fail closed (`CONTROL_TOKEN_PERMS`). |
| Control info file | | `--control-info-file <PATH>` | `.libra/code/control.json` | Path for non-secret local endpoint discovery metadata. Written atomically at `0600` on Unix/macOS in launch modes. Never contains token material. With `--control stdio`, this is the **read** discovery path for `baseUrl` only (explicit `--control-url` overrides). Custom info paths do **not** relocate the default token — pass `--control-token-file` when the token is not under the worktree `code/` directory. |
| Control URL | | `--control-url <URL>` | (discovered) | Base URL of an existing Code UI control endpoint (e.g. `http://127.0.0.1:3000`). Only valid with `--control stdio`. When omitted, discovered from `--control-info-file`. Must be a literal loopback IP. |
| Provider | | `--provider` | `gemini` | AI provider backend (see Provider Backends below). |
| Model | | `--model` | provider default | Provider-specific model ID. |
| Agent profile | | `--agent <NAME>` | none | Select an agent profile by name. When the profile carries a structured `model: provider/model[@variant]` binding, that binding wins atomically -- provider, model ID, and variant all come from the profile, and a separately supplied `--model` is ignored to avoid hybrid pairs; profiles without a structured binding fall back to the CLI defaults. Profiles resolve through the three-tier hierarchy (project `.libra/agents/`, user `~/.config/libra/agents/`, embedded). Unknown or non-primary-eligible profiles are rejected. |
| Temperature | | `--temperature` | provider default | Sampling temperature for generation. |
| Ollama thinking | | `--ollama-thinking` / `--thinking` | `OLLAMA_THINK`, then `off` | Ollama thinking mode: `auto`, `off`, `on`, `low`, `medium`, or `high`. |
| Ollama compact tools | | `--ollama-compact-tools` | `OLLAMA_COMPACT_TOOLS`, then off | Sends compact tool schemas for remote/cloud Ollama endpoints that reject complex JSON schemas. |
| DeepSeek thinking | | `--deepseek-thinking <enabled\|disabled>` | omitted | Sends DeepSeek's `thinking` object when using `--provider deepseek`. |
| DeepSeek reasoning effort | | `--deepseek-reasoning-effort <low\|medium\|high\|max>` | omitted | Sends DeepSeek's `reasoning_effort` value when using `--provider deepseek`; `xhigh` is accepted as an alias for `max`. |
| DeepSeek stream | | `--deepseek-stream <true\|false>` / `--stream <true\|false>` | `false` | Sends DeepSeek's `stream` boolean when using `--provider deepseek`. |
| Kimi thinking | | `--kimi-thinking <enabled\|disabled>` | model default | Sends Kimi's `thinking` object when using `--provider kimi`. |
| Kimi stream | | `--kimi-stream <true\|false>` | `true` (Kimi) | Sends Kimi's `stream` boolean when using `--provider kimi`; defaults to streaming. Rejected for non-Kimi providers. |
| Context | | `--context` | none | Operating context: `dev` (alias `development`), `review` (alias `code-review`), `research` (alias `explore`). |
| Resume | | `--resume <THREAD_ID>` | none | Resume a canonical Libra thread by thread ID. |
| Approval policy | | `--approval-policy` | `on-request` | Tool approval policy (see Approval Policies below). |
| Approval TTL | | `--approval-ttl <SECS>` | `300` | Seconds that a granted approval stays reusable for matching commands before the agent is prompted again. Overrides the project config `[approval] ttl_seconds` in `.libra/config.toml`; relevant to the prompting policies. |
| Network access | | `--network-access <allow\|deny>` | `deny` | Default network policy for shell/gate under legacy TUI (`LIBRA_CODE_LEGACY_TUI=1`). Default Web and MCP `--stdio` reject `--network-access allow` until the Plan network-policy gate owns per-execution sandbox network (approve network in Plan review instead). |
| MCP port | | `--mcp-port` | `6789` | MCP server listen port. |
| Stdio | | `--stdio` / `--mcp-stdio` | off | Deprecated MCP-only legacy: tools/resources over stdio (not turn control). Prefer `--control stdio` for automation; dedicated `libra mcp --stdio` planned after W5. Conflicts with `--web-only`. |
| API base | | `--api-base` | provider default | Provider API base URL override. |
| Codex binary | | `--codex-bin` | `codex` | Codex executable path. |
| Codex port | | `--codex-port` | random | Override Codex app-server port. |
| Plan mode | | `--plan-mode [<true\|false>]` | off (on for Codex) | Require the agent to produce an approved plan before execution. The effective default is on for `--provider codex` and off for every other provider; Explicit `--plan-mode=true` (or bare `--plan-mode`) is only accepted with `--provider=codex` — it is rejected for other providers; pass `--plan-mode=false` to opt a Codex session out. |
| Browser control | | `--browser-control <off\|loopback>` | `loopback` (Web); `off` (legacy TUI) | Posture for `/api/code/controller/attach` browser leases. Conflicts with `--stdio`; `loopback` requires a loopback `--host`. |
| Goal | | `--goal <OBJECTIVE>` | none | Boot the session in goal mode with the supplied objective, equivalent to running `/goal start <objective>` as the session opens; the supervisor drives the tool loop until completion is claimed and the verifier accepts. The objective is validated at parse time (non-empty after trim, at most 16 KiB). |

### Provider Backends

| Value | Description | API Key Env | Base URL Override |
|-------|-------------|-------------|-------------------|
| `gemini` | Google Gemini (default: gemini-2.5-flash) | `GEMINI_API_KEY` | `--api-base` |
| `openai` | OpenAI (default: gpt-4o-mini) | `OPENAI_API_KEY` | `--api-base`, `OPENAI_BASE_URL` |
| `anthropic` | Anthropic (default: claude-3.5-sonnet) | `ANTHROPIC_API_KEY` | `--api-base`, `ANTHROPIC_BASE_URL` |
| `deepseek` | DeepSeek | `DEEPSEEK_API_KEY` | `--api-base` |
| `kimi` | Moonshot AI Kimi (default: kimi-k2.6) | `MOONSHOT_API_KEY` | `--api-base`, `MOONSHOT_BASE_URL`, `--kimi-thinking` |
| `zhipu` | Zhipu GLM (default: glm-5) | `ZHIPU_API_KEY` | `--api-base`, `ZHIPU_BASE_URL` |
| `ollama` | Ollama (local models and direct Cloud API) | `OLLAMA_API_KEY` for direct Cloud API | `OLLAMA_BASE_URL`, `OLLAMA_THINK`, `OLLAMA_COMPACT_TOOLS`, `--api-base`, `--ollama-thinking`, or `--ollama-compact-tools` |
| `codex` | Codex app-server | -- | `--codex-bin` / `--codex-port` |

For Codex app-server linkage, model forwarding, credentials ownership, and persisted object storage details, see [Codex data storage integration](codex-data-storage.md).

DeepSeek requests can opt into provider-specific fields with `--deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true`; these flags are rejected for non-DeepSeek providers.
Kimi requests default to the selected model's thinking behavior; use `--kimi-thinking disabled` for K2.6/K2.5 runs where lower latency or official web-search compatibility matters. Libra preserves Kimi `reasoning_content` across tool-call turns when the provider returns it.
For normal runs, store provider keys in `vault.env.<NAME>`; Libra checks repo-local Vault, then global Vault, then the process environment. Use `--env-file .env.test` for live tests that need an explicit dotenv override. Under `--web`/`--web-only`, `--env-file`, `--context`, `--approval-policy`, and `--approval-ttl` keep the same semantics as TUI for non-Codex providers (env-file values still override process env/Vault). Managed `--provider codex` still rejects `--env-file`, `--approval-ttl`, and `--resume` because those surfaces are not wired into the Codex app-server path; MCP `--stdio` continues to reject the TUI-only flags.

Ollama requests stream `/api/chat` responses by default and add a per-request `request_id` to debug logs. They also default to `think:false` so reasoning-capable local models do not spend several minutes generating hidden reasoning before tool calls. Use `--ollama-thinking high` for a single run, or set `OLLAMA_THINK=true`, `low`, `medium`, `high`, or `auto` as the environment default. `auto` omits the `think` field and lets Ollama decide. Use `--ollama-compact-tools` or `OLLAMA_COMPACT_TOOLS=true` when a remote/cloud Ollama endpoint accepts simple tools but returns 503 for Libra's full tool schema payload.

### Local Automation Control

`libra code --control observe` is the default and does not create local control files unless `--control-info-file` is explicitly supplied. Loopback clients can continue reading `/api/code/session` and `/api/code/events` without a token.

`libra code --control write` enables the local automation security envelope. Libra creates a fresh 32-byte token in `.libra/code/control-token`, atomically writes non-secret endpoint metadata to `.libra/code/control.json` (Unix/macOS mode `0600`) after the web server binds, and holds `.libra/code/control.lock` for the process lifetime. Default paths are per worktree local-gitdir, so two worktrees never share a token/info/lock; a cross-worktree scope mismatch fail-closes rather than reclaiming another worktree's sidecar. `control.json` includes `version`, `mode`, `pid`, `baseUrl`, optional `mcpUrl`, `workingDir`, optional `threadId`, `startedAt`, and version-2 writer scope (`repoId`/`worktreeId`/optional `workspaceId`/`leaseFence`); it never includes the token, token hash, token path, provider credentials, headers, or provider request/response bodies.

Write control is local-only. `--control write` is rejected with `--stdio`, and it requires `--host` to be loopback (`127.0.0.1`, `::1`, or `localhost`). A second write-control instance using the same default paths fails fast with `CONTROL_INSTANCE_CONFLICT`; use distinct `--control-token-file` and `--control-info-file` paths only when the caller intentionally manages multiple local instances.

Automation clients attach with `POST /api/code/controller/attach`, body `{ "clientId": "...", "kind": "automation" }`, header `X-Libra-Control-Token`, and then use the returned `X-Code-Controller-Token` for writes. Automation-held leases require both tokens for `/api/code/messages`, `/api/code/interactions/{id}`, `/api/code/controller/detach`, and `/api/code/control/cancel`. The local TUI can reclaim control with `/control reclaim`, which invalidates the automation lease. Code UI write request bodies are capped at 256KiB. A plan-repair Continue that raises an exhausted retry limit sends `{ "selectedOption": "continue", "maxAttempts": 3 }`; `maxAttempts` must exceed the current limit and not exceed 10. When the session advertises `capabilities.commandIdempotency` (headless web-only today), `POST /api/code/messages` accepts `{ "text": "...", "commandId": "..." }` for retry de-duplication (same id + same text is idempotent; same id + different text returns `COMMAND_PAYLOAD_CONFLICT`). The runtime namespaces each `commandId` under a SHA-256 fence of the active controller `clientId` before durable admission (the raw clientId is never written into the command log). `commandIdempotency` is advertised only when durable SessionStore command admission is configured.

`GET /api/code/diagnostics` returns a redacted observe-only status summary for local tools. Control attach, detach, submit, respond, and cancel operations emit `local-tui-control/v1` audit events through the runtime audit sink. For stdio automation clients, prefer the canonical `libra code --control stdio` JSON-RPC NDJSON client: it discovers the endpoint from `.libra/code/control.json` by default (override with `--control-url` / `--control-token-file` / `--control-info-file`). Discovery fails closed with stable codes (`CONTROL_INFO_MISSING`, `CONTROL_INFO_PERMS`, `CONTROL_TOKEN_MISSING`, `CONTROL_TOKEN_PERMS`, `CONTROL_SCOPE_CONFLICT`, `CONTROL_SERVER_MISSING`); attach lease/ownership conflicts surface as JSON-RPC `-32000` with Libra codes such as `CONTROLLER_CONFLICT`. The legacy [`libra code-control --stdio`](code-control.md) shim remains available until W4-09. Deprecated `libra code --stdio` remains the **MCP-only** tools/resources transport (stderr deprecation warning; not turn control) and must not be confused with `--control stdio`; a dedicated `libra mcp --stdio` is planned after W5.

### Web Browser Control

`--browser-control <off|loopback>` controls whether the embedded UI's lease-based write surface is available. The default is mode-aware:

| Entry point | Default `--browser-control` |
|-------------|-----------------------------|
| TUI session (`LIBRA_CODE_LEGACY_TUI=1`) | `off` |
| Default Web / `--web` / `--web-only` (any provider) | `loopback` |

Selecting `loopback` is rejected when `--host` is not a loopback address, and the flag conflicts with `--stdio`. Use `--browser-control off` when binding a non-loopback `--host` for observe-only / remote-notice serving.

**Local trust model:** browser attach requires loopback bind + trusted same-origin `Origin`/`Referer` + rate limits (W3-05) **and** a per-session `X-Libra-Browser-Bootstrap` secret (printed on stdout / embedded as `?bt=` in the open URL). Forgeable Origin alone is not enough. Libra does **not** auto-open a `?bt=` URL (so the bootstrap secret never appears in opener argv on shared hosts); open the printed URL yourself. On shared machines, prefer `--browser-control off` (observe-only) or keep the session on a private host.

The browser server-side endpoints are tagged in the `code_router()` audit matrix (`src/internal/ai/web/mod.rs`):

- `GET /api/code/session`, `GET /api/code/events`, `GET /api/code/diagnostics`, `GET /api/code/threads`, `GET /api/code/usage`, `GET /api/code/skills`, `GET /api/code/goal/status` — loopback-only observe.
- `POST /api/code/controller/attach` — loopback. `kind: "automation"` requests additionally require `X-Libra-Control-Token`. The handler **issues** the lease's `controllerToken` (it does not expect the caller to send one).
- `POST /api/code/controller/detach`, `POST /api/code/messages`, `POST /api/code/interactions/{id}` — loopback + `X-Code-Controller-Token`; `Automation` leases additionally require `X-Libra-Control-Token`.
- `POST /api/code/control/cancel` — loopback + `X-Code-Controller-Token`. `Automation` leases also require `X-Libra-Control-Token`; this is the only difference from the TUI `Esc` cancel path.
- `POST /api/code/task/dispatch` — loopback + `X-Code-Controller-Token`; user-initiated sub-agent dispatch requires an active controller write lease (browser or automation). Automation leases additionally require `X-Libra-Control-Token`.
- `POST /api/code/goal/start`, `POST /api/code/goal/cancel` — loopback + `X-Code-Controller-Token`; goal mutation requires the active controller lease.
- `POST /api/code/skills/activate`, `POST /api/code/session/resume` — loopback + `X-Code-Controller-Token` on the write router (256 KiB body limit); both require an active controller write lease. Automation leases additionally require `X-Libra-Control-Token`. Resume refuses busy and indeterminate snapshots, and currently fail-closes with `SESSION_RESUME_REQUIRES_RESTART` after proving the target thread is loadable (in-process AgentRuntime swap is not available yet). Skill activate fail-closes with `SKILL_ACTIVATION_UNSUPPORTED` after discoverability validation until a provider-consumed activation path exists.

Browser write requests share the same 256 KiB body limit and audit-sink wiring as automation control. The browser persists the lease only in memory; reloading the page drops the lease and the next write reattaches.

Browser writes (including `POST /controller/attach` with `kind: "browser"`) additionally require a trusted loopback `Origin` (or same-origin `Referer` fallback) that matches the Code UI bind address (exact `http://<bound-ip>:<port>`, plus `localhost` / `127.0.0.1` / `[::1]` aliases when bound to canonical loopback). Missing or cross-site Origin fails closed with `ORIGIN_REQUIRED`. Automation writes authenticate with `X-Libra-Control-Token` / controller lease and do **not** use Origin as a substitute. Per-session write rate limiting applies to both browser and automation producers (`LIBRA_CODE_SESSION_WRITE_RATE_LIMIT` / `LIBRA_CODE_SESSION_WRITE_RATE_WINDOW_SECS`, default 120 writes / 60s) and returns `429 RATE_LIMITED` until the window recovers.

The embedded SPA session-lifecycle panels list threads via `GET /api/code/threads`, cancel the active turn through `POST /api/code/control/cancel` (fail-closed when `controller.canWrite` is false), and post resume selection through `POST /api/code/session/resume` with `{ "threadId": "..." }`. Thread list is repository-storage-scoped (shared across linked worktrees), while resume is working-directory scoped; listed items omit `workingDir` until ThreadProjection persists a per-thread cwd.

The usage panel mirrors the W2-12 `RuntimeUsageTotals` read model (cumulative, current-turn delta, sub-agent attribution) and keeps `partial`/`unknown`/`error` visible instead of pretending zero spend. `GET /api/code/usage` reads durable totals and returns an error rather than fabricated zeroes. When durable sub-agent enumeration is unavailable, the response omits `subAgents` and sets `subAgentsStatus: "unavailable"` instead of an empty array.

The execution/repair panel projects `plans[]`, `toolCalls[]`, and `planExecutionRepair` from the live session snapshot. Continue/Cancel post through `POST /api/code/interactions/{id}` with `selectedOption` (`continue` / `cancel`); when projected `attempt >= max_attempts`, Continue also sends a raised `maxAttempts` (capped at 10) without reclassifying the failure on the client.

The SSE resilience panel surfaces reconnecting / resync-required / resynced status while keeping the last projected session snapshot and the last wire-supplied cursor seq (the browser never invents sequence numbers). Explicit snapshot resync routes through the shared store `refresh()` path and only reports success when that refresh applies (or is superseded by a newer live update). Production v2 transport backlog/resync (`event: resync` / `WIRE_V2_RESYNC_REQUIRED`) is delivered by W3-08; the built-in SPA cutover to consume it remains W3-09.

The workflow review panel projects pending `intent_review_choice` and `post_plan_choice` interactions (network policy is the same kind with `metadata.phase = "networkPolicy"`). Confirm/modify/cancel (and execute / network-allow / network-deny / back) post `selectedOption` through the leased interaction endpoint; turn cancel is fail-closed when the browser cannot write. The panel does not keep a second workflow FSM — it waits for the next snapshot/SSE update.

When the server is bound to a non-loopback host (`--host 0.0.0.0` or a LAN address), non-loopback browsers receive a static remote access notice for HTML navigation instead of the SPA. The notice is zero JavaScript, includes only bind/remote/version/commit placeholders, and asset/API fallbacks return 404 so remote clients cannot probe session state. Snapshot, transcript, SSE, approval, and every `/api/code/*` read/write surface stay loopback-only (`LOOPBACK_REQUIRED`). Remote humans should SSH port-forward to loopback (`ssh -L 3000:127.0.0.1:3000 user@host`) rather than expecting a direct remote write UI; authenticated TLS reverse proxies are deferred (DEFER-04) and are not the default.

Default listen port is `3000`. If that address is already bound, startup fail-closes with an actionable `--port` hint and never auto-scans the next free port.

When `--browser-control loopback` is requested and the browser holds the active lease, the TUI initial controller is `LocalTui` (visible owner, can be reclaimed) instead of `Fixed { Tui }` (permanently blocking). If the TUI also wants to drive writes, `--control write` must be supplied alongside `--browser-control loopback`; the two writers serialize through the same `TuiControlCommand` channel.

For `--web-only` non-Codex providers (`--provider ollama` is the canonical headless verification path), Libra builds a [`HeadlessCodeRuntime`](../../src/internal/ai/web/headless.rs) lifecycle host and mounts [`AgentRuntimeCodeUiAdapter`](../../src/internal/ai/web/agent_runtime_adapter.rs) as the production browser write-path owner. Browser submits enter the serialized `AgentRuntimeWorker`: plain (non-`/`) messages use the TUI-equivalent Phase 0 plan tool allowlist so direct chat cannot bypass the default mutating gate; slash/`/`-prefixed messages keep an explicit direct tool loop. Full IntentSpec → Phase 1 → repair parity remains **GATE-WEB-PLAN** (intentional residual through W4-01; not a silent cutover regression — use `LIBRA_CODE_LEGACY_TUI=1` when the full TUI Phase 0/1/repair workflow is required). Headless mode advertises `messageInput`, `streamingText`, `toolCalls`, `planUpdates`, `patchsets`, `interactiveApprovals`, `structuredQuestions`, and `providerSessionResume`. `--web-only --resume <thread_id>` (and bare default Web `--resume`) reloads the matching session for non-Codex providers in the same working directory, then applies the bounded durable Code UI projection suffix before starting the browser server. `--resume` remains unavailable with explicit `--web`/`--web-only --provider codex`; bare `libra code --provider codex --resume <thread_id>` keeps the pre-W4 legacy TUI resume driver until managed Codex Web resume lands. `update_plan` projects into `plans[]`, and `apply_patch` metadata projects into `patchsets[]`. Cancellation is cooperative before a tool's mutation boundary. After a potentially mutating tool has begun, cancel returns an actionable error and the active turn remains in place until it reaches a determinate result; Libra never hard-aborts that side effect or relabels it as an ordinary cancelled turn.

For `--web-only --provider codex`, managed app-server websocket notifications are normalized into the shared runtime `AgentEvent` envelope (same projection path as other providers). Unknown Codex methods take an explicit diagnosable `ProviderNotification` fallback rather than silent drop or panic. Ask-mode approvals park on the shared `AgentRuntime` interaction registry and forward browser `respond_interaction` decisions to the app-server; Codex still owns the in-app-server approval loop (see DEFER-07 in `docs/development/tracing/code.md`). Outward approval option ids match non-Codex (`approve` / `deny` / `abort`).

### Code UI Wire Contract

The Code UI JSON contract uses camelCase field names and snake_case enum values. The Rust source of truth is `src/internal/ai/web/code_ui.rs`; the browser mirror is `web/src/lib/code-ui/types.ts`; `tests/ai_code_ui_wire_test.rs` pins the wire shape.

`GET /api/code/session` returns a `CodeUiSessionSnapshot`:

| Field | Type | Contract |
|-------|------|----------|
| `sessionId` | string | Runtime session identifier retained for compatibility. |
| `threadId` | string, optional | Canonical persisted Libra thread ID; prefer this for resume, graph, Web, MCP, and diagnostics flows when present. |
| `workingDir` | string | Session working directory. |
| `provider` | object | `{ provider, model?, mode?, managed }`. |
| `capabilities` | object | Eight booleans: `messageInput`, `streamingText`, `planUpdates`, `toolCalls`, `patchsets`, `interactiveApprovals`, `structuredQuestions`, `providerSessionResume`. |
| `controller` | object | `{ kind, ownerLabel?, canWrite, leaseExpiresAt?, reason?, loopbackOnly }`; `kind` is `none`, `browser`, `automation`, `tui`, or `cli`. |
| `status` | string | `idle`, `thinking`, `executing_tool`, `awaiting_interaction`, `completed`, `error`, or `indeterminate_side_effect`. The final value means a mutating command may have taken effect and must be reconciled before any retry. |
| `transcript` | array | Entries with `id`, `kind`, optional `title` / `content` / `status`, `streaming`, `metadata`, `createdAt`, `updatedAt`. |
| `plans` / `tasks` / `toolCalls` / `patchsets` | arrays | Runtime projections used by Workflow, Summary, Diff, and Terminal panes. |
| `interactions` | array | Pending/resolved UI prompts. `kind` is `approval`, `sandbox_approval`, `request_user_input`, `intent_review_choice`, `post_plan_choice`, or `plan_execution_repair`. A pending plan-repair interaction offers `continue` and `cancel`; respond through the normal interaction endpoint. |
| `planExecutionRepair` | object, optional | Runtime-owned plan-execution repair state. It contains a snake_case `state`, bounded and runtime-redacted failure `evidence` (`output`, `diagnostics`, `attempt`, `max_attempts`), and an `interaction_id` while `awaiting_user`. `automatic_repair` records an in-progress retry. `awaiting_user` is projected only after the configured retries are exhausted: a Code UI Continue must send a higher `maxAttempts` (for example, `{ "selectedOption": "continue", "maxAttempts": 3 }`), otherwise it returns `PLAN_REPAIR_RETRY_LIMIT_REACHED`; alternatively, provide manual revision guidance. Cancel is terminal. `intent_spec_revision` and `manual_action` require a new user-directed workflow. |
| `updatedAt` | string | ISO 8601 update timestamp. |

`GET /api/code/events` streams session updates. Wire version is negotiated as follows
(W3-06 / plan-20260715):

| Selection | Mechanism |
|---|---|
| Explicit v1 | `?wire=1` or `?wire=v1` |
| Explicit v2 | `?wire=2` or `?wire=v2` |
| Accept hint | `Accept: text/event-stream;libra-wire=2` (query `wire=` wins if both are set) |
| Default (unspecified) | **v1** for clients that omit `wire` / `libra-wire`. The built-in SPA (W3-09) always requests `?wire=2`. |
| Illegal values | fail-closed `400 INVALID_WIRE_VERSION` |

**SSE v1** (default): `CodeUiEventEnvelope` records with `seq`, `type`, `at`, and
`data`. Event `type` is `session_updated`, `status_changed`, or
`controller_changed`; `session_updated` carries a full `CodeUiSessionSnapshot`.

**SSE wire v2**: `code_workflow` events with camelCase `cursor` (durable W1-06
workflow sequence), `eventId`, `kind`, `at`, and minimal `payload`. Reconnect with
`?wire=2&cursor=<lastCursor>` to replay without duplicates or gaps inside the
**transport** backlog window (W3-08 / GC-CODE-12): **1,024 events or 8 MiB**,
whichever is reached first (`MAX_CODE_UI_TRANSPORT_BACKLOG_*`). The Code UI
**projection** hot window is a separate budget with the same numeric caps
(`MAX_CODE_UI_PROJECTION_EVENTS` / `MAX_CODE_UI_PROJECTION_REPLAY_BYTES`);
do not add the two together. Single-event folds visit only the suffix, not
the full session history (W3-14; release p95 ≤ 5 ms on 10k-event sessions). When bootstrap or a lagged consumer would
exceed that budget, the server emits `event: resync` with
`WIRE_V2_RESYNC_REQUIRED` (`reason`, `lastCursor`, `durableTail`,
`action: fetch_snapshot`) and ends the stream — never silent-drops. Clients
fetch a session snapshot, then reconnect at `durableTail`. Wire v2 requires a
SessionStore-backed workflow hub. Today that hub is mounted for default Web /
`--web-only` headless runs with session persistence (non-Codex
`HeadlessCodeRuntime`). Legacy TUI + background web and managed
`--provider codex` Web currently return `503 WIRE_V2_REQUIRES_DURABLE_SESSION`
until those runtimes expose a hub.

### SSE v1 compatibility window (DEFER-08)

v1 snapshot SSE remains supported through at least one successful public patch
release after wire v2 becomes the default and the built-in frontend/automation
clients have migrated. Physical removal of v1 is **not** part of plan-20260715;
see DEFER-08 / ADR-CODE-08. Removal preconditions (checklist; all required):

1. Built-in frontend migrated to v2 (W3-09 evidence): the SPA
   opens `GET /api/code/events?wire=2` from `sse-resilience` (`wrapClientForSseResilience`),
   reconnects with `cursor` from the wire, and treats `event: resync` /
   `WIRE_V2_RESYNC_REQUIRED` as one explicit snapshot pull (W2-15 UI). Cursor/seq
   are never invented client-side.
2. Built-in automation clients migrated to v2.
3. Compat / matrix tests consume v2 by default.
4. Release notes name the last v1-supporting version and the upgrade path.
5. At least one successful public patch release after (1)–(4) while v1 still works.


`GET /api/code/threads` returns `{ items, nextOffset? }`. Each item has `id`, optional `title`, `archived`, optional `currentIntentId`, optional `workingDir`, `createdAt`, and `updatedAt`. `workingDir` is omitted until ThreadProjection persists a per-thread cwd (do not invent the server cwd for linked-worktree threads). `limit` defaults to 50 and clamps to 200; malformed `limit` or `offset` returns `INVALID_QUERY_PARAM`.

`GET /api/code/skills?provider=<slug>&skill=<name>` returns curated A0-07 `{ items: [{ name, provider }] }`. An unknown `provider` slug returns `INVALID_SKILL_PROVIDER` (same contract as activate); omit `provider` to list all curated providers. `POST /api/code/skills/activate` accepts `{ provider, name }`; after discoverability validation it currently returns `SKILL_ACTIVATION_UNSUPPORTED` until an in-process provider activation path exists.

Code UI API errors use `{ error: { code, message } }`:

| Code | HTTP | Meaning |
|------|------|---------|
| `LOOPBACK_REQUIRED` | 403 | Non-loopback client attempted an API route. |
| `PAYLOAD_TOO_LARGE` | 413 | Write request body exceeded 256 KiB. |
| `ORIGIN_REQUIRED` | 403 | Browser write/attach lacked a trusted loopback `Origin` (or same-origin `Referer`), or presented a cross-site Origin. |
| `MISSING_BROWSER_BOOTSTRAP` | 403 | Browser attach lacked `X-Libra-Browser-Bootstrap` for a session that minted a bootstrap secret. |
| `INVALID_BROWSER_BOOTSTRAP` | 403 | `X-Libra-Browser-Bootstrap` does not match this Libra Code session. |
| `RATE_LIMITED` | 429 | Per-session write budget exhausted; retry after the rate-limit window (see `Retry-After` / wait for window recovery). |
| `REDACTION_FAILED` | 500 | Session / diagnostics / SSE projection could not apply the secret redactor (empty rules or serialize failure). Fail closed: the response omits unredacted payload; restart `libra code` or retry after fixing redactor configuration. |
| `INVALID_WIRE_VERSION` | 400 | `GET /api/code/events` wire negotiation received an illegal `wire` / `libra-wire` value (only `1`/`v1` and `2`/`v2` are accepted). |
| `WIRE_V2_REQUIRES_DURABLE_SESSION` | 503 | SSE wire v2 requires a SessionStore-backed workflow hub (mounted today for `--web-only` headless persistence; TUI background web and managed Codex web-only do not yet expose one). |
| `WIRE_V2_CURSOR_AHEAD` | 409 | `?cursor=` is ahead of the durable workflow tail; drop the cursor and resync (an ahead cursor would permanently skip live events). |
| `WIRE_V2_RESYNC_REQUIRED` | SSE `resync` then close | Transport backlog exceeded (1,024 events / 8 MiB); fetch snapshot and reconnect with `cursor=<durableTail>`. |
| `WIRE_V2_REPLAY_FAILED` | 500 | Wire v2 could not replay durable workflow events after the requested cursor (gap or I/O; capacity exits use `WIRE_V2_RESYNC_REQUIRED`). |
| `CONTROL_DISABLED` | 403 | Automation control is not enabled for this process. |
| `MISSING_CONTROL_TOKEN` | 403 | Automation control token is absent. |
| `INVALID_CONTROL_TOKEN` | 403 | Automation control token is invalid. |
| `MISSING_CONTROLLER_TOKEN` | 403 | Lease token is absent for a write route. |
| `INVALID_CONTROLLER_TOKEN` | 403 | Lease token is invalid or stale for a write route. |
| `INVALID_CONTROLLER_KIND` | 400 | Controller attach requested an unsupported kind. |
| `CONTROLLER_CONFLICT` | 409 | Another live controller owns the lease, or the session is busy. |
| `INTERACTION_NOT_ACTIVE` | 409 | Respond targeted an interaction with no active runtime turn. |
| `BROWSER_CONTROL_DISABLED` | 403 | Browser write control is disabled. |
| `AUTOMATION_CONTROLLER_REQUIRED` | 403 | An automation-only path was called with a non-automation lease. |
| `CODE_UI_UNAVAILABLE` | 404 | No active `libra code` session is attached to the web server. |
| `INVALID_QUERY_PARAM` | 400 | Query parsing failed, currently for `/threads` pagination. |
| `INVALID_COMMAND_ID` | 400 | `commandId` was empty, too long, or contained whitespace/control characters. |
| `STORAGE_PATH_INVALID` | 500 | Storage-root resolution failed. |
| `STORAGE_ROOT_UNRESOLVED` | 500 | Repository storage root could not be resolved. |
| `STATUS_UNAVAILABLE` | 500 | Runtime status snapshot is unavailable. |
| `THREAD_LIST_FAILED` | 500 | Thread projection enumeration failed. |
| `DB_UNAVAILABLE` | 500 | Session database is offline. |
| `USAGE_UNAVAILABLE` | 500 | Durable runtime usage could not be queried. |
| `INVALID_SKILL_PROVIDER` | 400 | The requested skill provider is not an A0-07 agent slug. |
| `SKILL_NOT_DISCOVERABLE` | 400 | The requested skill is not curated for that provider. |
| `SKILL_ACTIVATION_UNSUPPORTED` | 422 | Skill is discoverable, but in-process activation is not available yet. |
| `SESSION_RESUME_BUSY` | 409 | A thinking or tool-running session cannot be replaced. |
| `SESSION_RESUME_NOT_FOUND` | 404 | No matching session exists under this working directory. |
| `SESSION_RESUME_REQUIRES_RESTART` | 422 | Target thread is loadable, but in-process AgentRuntime swap is not available; restart with `libra code --resume <threadId>`. |
| `SESSION_RESUME_LOAD_FAILED` | 500 | Target thread exists but session storage/checkpoint could not be loaded or folded. |
| `RECONCILIATION_REQUIRED` | 409 | A mutating turn needs manual reconciliation before another turn can run. |
| `COMMAND_PAYLOAD_CONFLICT` | 409 | The same `commandId` was reused with a different message payload. |
| `COMMAND_ALREADY_TERMINAL` | 409 | The same `commandId` already finished failed/cancelled/indeterminate; allocate a new `commandId` to retry. |
| `PLAN_REPAIR_RETRY_LIMIT_REACHED` | 409 | A plan-repair Continue request did not raise the exhausted automatic retry cap. Retry with a higher `maxAttempts` (for example, `{ "selectedOption": "continue", "maxAttempts": 3 }`), provide manual revision guidance, or cancel the repair. |
| `INTERNAL_ERROR` | 500 | Fallback internal failure. |
| `UNSUPPORTED_OPERATION` | 422 | Runtime rejected a requested operation that is not yet supported. |

### Web Search

The `web_search` tool requires the session network policy to allow outbound access. If `BRAVE_SEARCH_API_KEY` is available from `vault.env.BRAVE_SEARCH_API_KEY` or the process environment, Libra tries the Brave Search API first and returns result titles, URLs, and snippets. If Brave is not configured or the request fails, Libra falls back to the zero-configuration DuckDuckGo HTML endpoint.

### Approval Policies

| Value | Aliases | Description |
|-------|---------|-------------|
| `never` | -- | No prompts; dangerous commands are rejected outright. |
| `allow-all` | `allow_all`, `always`, `accept` | No prompts; every command is allowed for this session (`allows_all_commands`). |
| `on-failure` | `on-failure` | Prompt only when retrying after a sandbox denial. |
| `on-request` | `on-request` | Run inside sandbox by default; prompt when escalation or policy requires it (default). |
| `untrusted` | `unless-trusted`, `untrusted` | Prompt for non-trusted operations; auto-allow known-safe reads. |

### Context Modes

| Value | Aliases | Description |
|-------|---------|-------------|
| `dev` | `development` | General development workflow. |
| `review` | `code-review` | Code review focus. |
| `research` | `explore` | Exploratory research and analysis. |

## Common Commands

```bash
# Start a Web Code UI session with default Gemini provider
libra code

# Start with Anthropic Claude
libra code --provider anthropic --model claude-sonnet-4-20250514

# Bind web-only on all interfaces; remote browsers see a loopback-only notice
# (explicit --browser-control off: default Web is loopback and rejects non-loopback hosts)
libra code --web-only --port 8080 --host 0.0.0.0 --browser-control off

# Remote humans should SSH port-forward to the bound loopback port
# ssh -L 8080:127.0.0.1:8080 user@host
# then browse http://127.0.0.1:8080 locally

# Browser-driven session against a local Ollama (browser write lease is on by default)
libra code --web-only --provider ollama --port 4400

# Managed Codex on the default Web path (browser write lease is loopback by default)
libra code --web-only --provider codex

# Enable local automation write control (writes token + lease discovery files)
libra code --control write

# Drive an existing write-control session over JSON-RPC NDJSON (client-only).
# Defaults read `.libra/code/control.json` + sibling `control-token`.
libra code --control stdio

# Explicit endpoint overrides (still loopback-only)
libra code --control stdio \
  --control-url http://127.0.0.1:3000 \
  --control-token-file .libra/code/control-token

# Load provider keys from a dotenv-style file (overrides stale shell env vars)
libra code --env-file .env.test

# Deprecated MCP-only legacy (tools/resources; not turn control).
# Prefer --control stdio for automation; dedicated `libra mcp --stdio` after W5.
libra code --stdio

# Use DeepSeek with reasoning enabled
libra code --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true
libra code --env-file .env.test --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true

# Use Kimi (Moonshot AI) with the K2.6 default; opt out of thinking for lower latency
libra code --provider kimi
libra code --provider kimi --model kimi-k2-thinking --kimi-thinking enabled
libra code --provider kimi --model kimi-k2.6 --kimi-thinking disabled

# Use a local Ollama model; plain requests generate a reviewable plan first
libra code --provider ollama --model llama3 --api-base http://127.0.0.1:11434/v1

# Use compact tool schemas for a remote/cloud Ollama endpoint
libra code --provider ollama --model minimax-m2.7:cloud --api-base http://192.168.0.5:11434/v1 --ollama-compact-tools

# Enable high thinking for one Ollama run
libra code --provider ollama --model qwen3.6 --ollama-thinking high

# Capture provider/TUI diagnostics while using a local Ollama model
LIBRA_LOG='libra::internal::ai=debug,libra::internal::tui=debug' \
LIBRA_LOG_FILE=/tmp/libra-code.log \
libra code --repo=/Volumes/Data/linked --provider ollama --model gemma4:31b

# Resume a canonical Libra thread in the TUI or with a non-Codex headless Web server
libra code --resume 11111111-1111-4111-8111-111111111111
libra code --web-only --provider ollama --resume 11111111-1111-4111-8111-111111111111

# Inspect the same thread's version graph
libra graph 11111111-1111-4111-8111-111111111111

# Inspect a thread graph from outside that repository
libra graph 11111111-1111-4111-8111-111111111111 --repo /Volumes/Data/linked

# Start in code review context with strict approval
libra code --context review --approval-policy untrusted

# Use Codex with plan-before-execute mode
libra code --provider codex --plan-mode
```

## Human Output

Output is delivered through the Web UI (default), the legacy TUI (`LIBRA_CODE_LEGACY_TUI=1`), or MCP depending on the mode. Default Web mode prints URL / control details on stdout and stays resident until SIGINT/SIGTERM. Legacy TUI mode has no line-oriented stdout. In the generic provider workflow, a normal plain-text request starts the plan workflow automatically; explicit slash commands keep their command-specific behavior. Generic provider planning uses a two-step review: the LLM first drafts an IntentSpec for confirmation, then the confirmed IntentSpec is sent back to the LLM to generate a reviewable execution plan before any execution starts. If a confirmed plan executes and fails, or the orchestrator aborts before reaching a final decision, Libra feeds the failure evidence back into the planner, asks it to add or adjust repair steps, and automatically runs the revised plan up to the automatic repair threshold. After that threshold is reached, the TUI waits for the developer to continue with a higher retry limit (for example, `/plan continue <higher-limit>`) or provide explicit plan repair guidance; a plain `continue` retains the exhausted limit and returns `PLAN_REPAIR_RETRY_LIMIT_REACHED`. Cancel is terminal. The web server serves an embedded Next.js application. The stdio mode communicates via JSON-RPC messages following the Model Context Protocol.

## Diagnostics

`libra code` supports tracing through `RUST_LOG` or `LIBRA_LOG`; when both are set, `LIBRA_LOG` takes precedence. For TUI sessions, prefer `LIBRA_LOG_FILE=<path>` so diagnostics are written to a plain log file instead of the alternate-screen terminal. When `LIBRA_LOG_FILE` is set without an explicit log filter, Libra defaults to `libra=debug`.

For Ollama provider failures, useful diagnostics are:

```bash
mkdir -p /tmp/libra-logs
LIBRA_LOG='libra::internal::ai=debug,libra::internal::tui=debug' \
LIBRA_LOG_FILE=/tmp/libra-logs/libra-code-ollama.log \
libra code --repo=/Volumes/Data/linked --provider ollama --model gemma4:31b
```

If the TUI reports an Ollama 503, also capture the local server state:

```bash
ollama ps >> /tmp/libra-logs/libra-code-ollama.log
ollama list >> /tmp/libra-logs/libra-code-ollama.log
```

## Design Rationale

### Why a TUI + web server hybrid?

The default Web Code UI is the primary collaborative surface. Legacy TUI (`LIBRA_CODE_LEGACY_TUI=1`) still provides a low-latency, keyboard-driven interface for terminal users. `--web` / `--web-only` are deprecated aliases for the default Web path (removed in W5-07).

### Why multiple AI provider support?

Different providers excel at different tasks and have different cost/latency profiles. Gemini is the default for its generous free tier and fast response times. Anthropic Claude excels at careful reasoning and code review. Local Ollama support enables fully offline development. By abstracting behind a `CompletionClient` trait, adding a new provider requires only implementing the trait without touching the session, tool, or TUI layers.

### Why MCP integration?

The Model Context Protocol (MCP) is an open standard for connecting AI clients to tool servers. Deprecated `libra code --stdio` still lets Libra act as an MCP tool/resource server for clients like Claude Desktop (tools/resources only — not live Code turn control). A dedicated `libra mcp --stdio` is planned after W5 (DEFER-02); until then this legacy entry prints a deprecation warning. Prefer `libra code --control stdio` for local automation against a write-control Web session. Libra exposes an allowlisted `run_libra_vcs` tool for version-control operations -- `status`, `diff`, `branch`, `log`, `show`, `show-ref`, `ls-files`, `add`, `commit`, and `switch` -- so external AI agents use Libra directly instead of invoking Git. `run_libra_vcs` only accepts those Libra subcommands; it is not a Git-compatible shell. For repository state inspection, prefer `status --json` or `status --porcelain v2 --untracked-files=all`, and use `ls-files` for tracked and untracked repository path inspection (for example `ls-files --others --exclude-standard` for ignore-aware untracked files). Libra-managed execution also rejects direct `git` shell commands.

### Why approval policies?

AI agents executing shell commands on a developer's machine present real safety risks. The five-tier approval system balances productivity with control:
- `never` is for fully locked-down environments where the agent can only read.
- `allow-all` is the opposite extreme: no prompts and every command runs, for trusted throwaway or sandboxed environments where friction outweighs risk.
- `on-failure` lets the agent try sandboxed execution and only asks when it fails.
- `on-request` (default) sandboxes everything and escalates when the agent or sandbox policy requires it.
- `untrusted` is the most conservative interactive mode, prompting for anything beyond known-safe reads.

### Why session persistence and resume?

Long coding sessions accumulate significant context: file edits, conversation history, tool outputs. Losing this context on an accidental terminal close is painful. Session persistence stores the full conversation and tool state, and `--resume <thread_id>` restores a canonical Libra thread.

The embedded Code UI exposes the same canonical identifier as `threadId` in its session snapshot. Older `session_id` fields remain present for compatibility, but new integrations should key resume, Web, MCP, and diagnostics flows by `threadId`.

For a persistent non-Codex Web session, the initial session write is a prerequisite for starting a turn: if it fails, Libra starts no turn and the browser can repair storage and retry. A later persistence failure changes the live session to `indeterminate_side_effect` and blocks further submits or interaction replies; inspect the durable session data before restarting or reconciling it.

On `Ctrl-C` or `SIGTERM`, a non-Codex headless or web-only process closes browser command admission, then runs the shared process lifecycle shutdown owner (runtime/listeners/managed child/control) under one deadline. Read-only/model work is cooperatively cancelled; a started mutating tool is allowed to finish within that budget. If the deadline expires, `libra code` exits with an explicit shutdown failure and requires session inspection and reconciliation before restart. Supervisors should prefer `SIGTERM` (or `Ctrl-C` / `SIGINT`) over `SIGKILL` so ports, leases, and child processes are released cleanly.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Interactive AI session | `libra code` | Not available | Not available |
| TUI mode | Default | Not available | Not available |
| Web mode | `--web-only` | Not available | Not available |
| MCP/stdio mode | `--stdio` | Not available | Not available |
| AI provider selection | `--provider` | Not available | Not available |
| Session resume | `--resume <thread_id>` | Not available | Not available |
| Tool approval policy | `--approval-policy` | Not available | Not available |

Note: Neither Git nor jj have an equivalent to `libra code`. This command represents Libra's core differentiation as an AI-agent-native version control system. The closest analogs in the Git ecosystem are third-party tools like GitHub Copilot CLI or aider, which are separate applications rather than integrated VCS commands.

## Error Handling

| Scenario | Behavior | Exit |
|----------|----------|------|
| `--web-only` and `--stdio` both specified | Clap argument conflict error | non-zero |
| Missing API key for selected provider | Fatal error with provider name and expected env var | non-zero |
| Port already in use | Fatal error naming `host:port` and instructing an explicit `--port` (no auto-scan) | non-zero |
| No terminal available in TUI mode | Falls back or reports error | non-zero |
| Thread ID not found on resume | Fatal error with canonical `thread_id` | non-zero |
| `--control write --stdio` | Usage error; MCP `--stdio` (tools/resources) and `--control stdio` automation are separate modes | non-zero |
| `--control write --host 0.0.0.0` or other non-loopback host | Usage error; write control is loopback-only | non-zero |
| Another live `--control write` owns the same control lock | `CONTROL_INSTANCE_CONFLICT` with existing PID/URL when available | non-zero |
| Control token file is a symlink, non-regular file, or not `0600` on Unix/macOS | Fatal setup error before the web server starts | non-zero |
