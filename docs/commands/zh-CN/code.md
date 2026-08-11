# `libra code`

启动带 TUI、Web 或 MCP 模式的交互式 AI 编码会话。

## 概要

```
libra code
libra code --web-only [-p <PORT>] [--host <HOST>]
libra code --stdio
libra code --provider <PROVIDER> [--model <MODEL>]
libra code --resume <THREAD_ID>
libra graph <THREAD_ID> [--repo <PATH>]
```

## 说明

`libra code` 启动一个交互式编码会话，让人类开发者与 AI agent 协作。默认模式会启动基于 ratatui/crossterm 的终端 UI（TUI），并带有后台 Web 服务器。Generic provider TUI 中的普通开发请求会先路由到内置 planning workflow：Libra 生成可审阅的 IntentSpec 和执行计划，然后在运行 mutating tools 之前等待 Execute Plan / Network / Modify Plan / Cancel。另有两种替代模式：`--web-only` 只运行 Web 服务器而不启动 TUI（适合浏览器访问或远程托管），`--stdio` 通过标准输入/输出运行 MCP server，用于与 Claude Desktop 等 AI clients 集成。

该命令支持八种 AI provider 后端（Gemini、OpenAI、Anthropic、DeepSeek、Kimi、Zhipu、Ollama、Codex），以及三种运行上下文（dev、review、research），用于针对不同工作流调节 agent 行为。会话可以通过 Libra 规范的 `--resume <thread_id>` 流程持久化和恢复。传入 `--goal "<objective>"` 会直接以 goal 模式启动会话，由 supervisor 驱动 tool loop 朝既定 objective 前进，直到 verifier 接受完成。

沙箱化工具执行层会强制 approval policies，控制 agent 何时可以运行 shell 命令、应用补丁、Web 搜索或执行其他可能破坏性的操作。TUI dev 会话默认使用 workspace-write 执行且禁止网络访问。执行计划就绪后，Plan review 对话框提供 Execute Plan / Modify Plan / Cancel；选择 Execute 后会再打开独立的强制 network-policy 提示（`Network: Deny` / `Network: Allow` / `Back`），只有该 gate 解决后才会应用网络策略，且两个 gate 都可在崩溃后恢复。Review 和 research 上下文保持只读，且不授予网络访问。

当 TUI 退出且 Libra 能推导出规范 thread ID 时，`libra code` 会打印后续 `libra graph <thread_id>` 命令，以便在独立 TUI 中检查该线程的 Intent/Plan/Task/Run/PatchSet 版本图。检查非当前目录仓库时，使用 `libra graph <thread_id> --repo <path>`。

**Linked worktree**：在统一的 Code/Agent 配置 resolver 落地前，`libra code`（所有模式）在 linked worktree 中一律拒绝启动——会话配置（agents、hooks、sandbox、审批）在该场景尚未按 worktree 解析，linked 会话可能在仓库 sandbox 策略下运行却缺失其安全 hooks。请改在 main worktree 运行；拒绝信息会给出该提示。同理，linked worktree 中 automation 的 VCS dispatch 被禁用（带 warning），`libra automation` 亦 fail-closed——见 [automation.md](automation.md)。

## 选项

| 标志 | 短参数 | 长参数 | 默认值 | 说明 |
|------|--------|--------|--------|------|
| Web only | | `--web-only` / `--web` | off | 只运行 Web 服务器，不运行 TUI。与 `--stdio` 冲突。 |
| Port | `-p` | `--port` | `3000` | Web 服务器监听端口。 |
| Host | | `--host` | `127.0.0.1` | Web 服务器 bind 地址。 |
| Working directory | | `--cwd` | 当前目录 | 会话工作目录。 |
| Env file | | `--env-file <PATH>` | 无 | 从 dotenv 风格文件加载 provider 环境变量；显式文件值优先于 Vault 和进程环境。 |
| Control mode | | `--control <observe\|write>` | `observe` | 本地自动化控制模式。`observe` 保留现有 loopback 读行为；`write` 启用本地 token discovery 和进程级自动化控制认证。 |
| Control token file | | `--control-token-file <PATH>` | `.libra/code/control-token` | 每进程本地自动化 token 路径。在 `write` 模式下，Unix/macOS 文件必须是权限 `0600` 的普通文件。 |
| Control info file | | `--control-info-file <PATH>` | `.libra/code/control.json` | 非 secret 本地 endpoint discovery 元数据路径。在 Unix/macOS 上以原子写 + `0600` 落盘。该文件永不包含 token 材料。 |
| Provider | | `--provider` | `gemini` | AI provider 后端（见下方 Provider Backends）。 |
| Model | | `--model` | provider 默认值 | Provider 专用 model ID。 |
| Agent profile | | `--agent <NAME>` | 无 | 按名称选择 agent profile。当 profile 携带结构化 `model: provider/model[@variant]` 绑定时，该绑定原子生效——provider、model ID 和 variant 全部来自 profile，单独提供的 `--model` 会被忽略以避免混搭组合；无结构化绑定的 profile 回退到 CLI 默认值。Profiles 通过三层层级解析（项目 `.libra/agents/`、用户 `~/.config/libra/agents/`、内置）。未知或非 primary-eligible profile 会被拒绝。 |
| Temperature | | `--temperature` | provider 默认值 | 生成采样 temperature。 |
| Ollama thinking | | `--ollama-thinking` / `--thinking` | `OLLAMA_THINK`，然后 `off` | Ollama thinking 模式：`auto`、`off`、`on`、`low`、`medium` 或 `high`。 |
| Ollama compact tools | | `--ollama-compact-tools` | `OLLAMA_COMPACT_TOOLS`，然后 off | 为拒绝复杂 JSON schemas 的远程/云 Ollama endpoint 发送紧凑 tool schemas。 |
| DeepSeek thinking | | `--deepseek-thinking <enabled\|disabled>` | 省略 | 使用 `--provider deepseek` 时发送 DeepSeek 的 `thinking` 对象。 |
| DeepSeek reasoning effort | | `--deepseek-reasoning-effort <low\|medium\|high\|max>` | 省略 | 使用 `--provider deepseek` 时发送 DeepSeek 的 `reasoning_effort` 值；`xhigh` 作为 `max` 的别名被接受。 |
| DeepSeek stream | | `--deepseek-stream <true\|false>` / `--stream <true\|false>` | `false` | 使用 `--provider deepseek` 时发送 DeepSeek 的 `stream` boolean。 |
| Kimi thinking | | `--kimi-thinking <enabled\|disabled>` | model 默认值 | 使用 `--provider kimi` 时发送 Kimi 的 `thinking` 对象。 |
| Kimi stream | | `--kimi-stream <true\|false>` | `true`（Kimi） | 使用 `--provider kimi` 时发送 Kimi 的 `stream` boolean；默认流式。对非 Kimi provider 拒绝。 |
| Context | | `--context` | 无 | 运行上下文：`dev`（别名 `development`）、`review`（别名 `code-review`）、`research`（别名 `explore`）。 |
| Resume | | `--resume <THREAD_ID>` | 无 | 按 thread ID 恢复规范 Libra 线程。 |
| Approval policy | | `--approval-policy` | `on-request` | 工具审批策略（见下方 Approval Policies）。 |
| Approval TTL | | `--approval-ttl <SECS>` | `300` | 已授予的审批在再次提示前对匹配命令保持可复用的秒数。覆盖 `.libra/config.toml` 中的项目配置 `[approval] ttl_seconds`；与会提示的策略相关。 |
| Network access | | `--network-access <allow\|deny>` | `deny` | shell 和 gate 执行的默认 TUI 网络策略；仍可在 Plan review 时切换。 |
| MCP port | | `--mcp-port` | `6789` | MCP server 监听端口。 |
| Stdio | | `--stdio` / `--mcp-stdio` | off | 通过 stdio 运行 MCP。与 `--web-only` 冲突。 |
| API base | | `--api-base` | provider 默认值 | Provider API base URL 覆盖。 |
| Codex binary | | `--codex-bin` | `codex` | Codex 可执行文件路径。 |
| Codex port | | `--codex-port` | 随机 | 覆盖 Codex app-server 端口。 |
| Plan mode | | `--plan-mode [<true\|false>]` | off（Codex 为 on） | 要求 agent 在执行前生成经批准的计划。有效默认值对 `--provider codex` 为 on，对其他所有 provider 为 off；显式 `--plan-mode=true`（或裸 `--plan-mode`）只在 `--provider=codex` 下被接受——对其他 provider 会被拒绝；传 `--plan-mode=false` 让 Codex 会话关闭计划模式（opt out of plan mode，非退出会话）。 |
| Browser control | | `--browser-control <off\|loopback>` | provider-aware（见 Web Browser Control） | `/api/code/controller/attach` 浏览器租约姿态。与 `--stdio` 冲突；`loopback` 要求 loopback `--host`。 |
| Goal | | `--goal <OBJECTIVE>` | 无 | 以 goal 模式启动会话并带上给定 objective，等价于会话打开时运行 `/goal start <objective>`；supervisor 驱动 tool loop 直到声明完成且 verifier 接受。objective 在解析时校验（trim 后非空，至多 16 KiB）。 |

### Provider Backends

| 值 | 说明 | API Key Env | Base URL 覆盖 |
|----|------|-------------|---------------|
| `gemini` | Google Gemini（默认：gemini-2.5-flash） | `GEMINI_API_KEY` | `--api-base` |
| `openai` | OpenAI（默认：gpt-4o-mini） | `OPENAI_API_KEY` | `--api-base`、`OPENAI_BASE_URL` |
| `anthropic` | Anthropic（默认：claude-3.5-sonnet） | `ANTHROPIC_API_KEY` | `--api-base`、`ANTHROPIC_BASE_URL` |
| `deepseek` | DeepSeek | `DEEPSEEK_API_KEY` | `--api-base` |
| `kimi` | Moonshot AI Kimi（默认：kimi-k2.6） | `MOONSHOT_API_KEY` | `--api-base`、`MOONSHOT_BASE_URL`、`--kimi-thinking` |
| `zhipu` | Zhipu GLM（默认：glm-5） | `ZHIPU_API_KEY` | `--api-base`、`ZHIPU_BASE_URL` |
| `ollama` | Ollama（本地模型和直接 Cloud API） | 直接 Cloud API 使用 `OLLAMA_API_KEY` | `OLLAMA_BASE_URL`、`OLLAMA_THINK`、`OLLAMA_COMPACT_TOOLS`、`--api-base`、`--ollama-thinking` 或 `--ollama-compact-tools` |
| `codex` | Codex app-server | -- | `--codex-bin` / `--codex-port` |

关于 Codex app-server 连接、model forwarding、credentials ownership 和持久化对象存储细节，见 [Codex data storage integration](codex-data-storage.md)。

DeepSeek 请求可以通过 `--deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true` 选择加入 provider 专用字段；这些标志会对非 DeepSeek provider 拒绝。
Kimi 请求默认使用所选 model 的 thinking 行为；对于需要更低延迟或官方 Web 搜索兼容性的 K2.6/K2.5 run，使用 `--kimi-thinking disabled`。当 provider 返回 Kimi `reasoning_content` 时，Libra 会在 tool-call turns 中保留它。
常规运行时，将 provider keys 存在 `vault.env.<NAME>` 中；Libra 先检查 repo-local Vault，再检查 global Vault，最后检查进程环境。对需要显式 dotenv 覆盖的 live tests，使用 `--env-file .env.test`。在 `--web`/`--web-only` 下，非 Codex provider 的 `--env-file`、`--context`、`--approval-policy`、`--approval-ttl` 与 TUI 语义一致（env-file 值仍优先于进程环境/Vault）。Managed `--provider codex` 仍拒绝 `--env-file`、`--approval-ttl` 与 `--resume`（未接入 Codex app-server 路径）；MCP `--stdio` 继续拒绝这些 TUI-only flag。

Ollama 请求默认流式读取 `/api/chat` 响应，并向 debug logs 添加每请求 `request_id`。它们也默认使用 `think:false`，避免具备 reasoning 能力的本地模型在 tool calls 前花数分钟生成隐藏 reasoning。单次运行使用 `--ollama-thinking high`，或将 `OLLAMA_THINK=true`、`low`、`medium`、`high` 或 `auto` 设为环境默认值。`auto` 会省略 `think` 字段并让 Ollama 决定。当远程/云 Ollama endpoint 接受简单 tools 但对 Libra 完整 tool schema payload 返回 503 时，使用 `--ollama-compact-tools` 或 `OLLAMA_COMPACT_TOOLS=true`。

### 本地自动化控制

`libra code --control observe` 是默认值，除非显式提供 `--control-info-file`，否则不会创建本地控制文件。Loopback clients 可以继续无 token 读取 `/api/code/session` 和 `/api/code/events`。

`libra code --control write` 启用本地自动化安全信封。Libra 会在 `.libra/code/control-token` 中创建新的 32-byte token，在 Web 服务器绑定后以原子写（Unix/macOS 模式 `0600`）将非 secret endpoint 元数据写入 `.libra/code/control.json`，并在进程生命周期内持有 `.libra/code/control.lock`。默认路径按 worktree 本地 gitdir 隔离，两个 worktree 不会共享 token/info/lock；跨 worktree 的 scope mismatch 会 fail-closed，而不是回收另一 worktree 的 sidecar。`control.json` 包含 `version`、`mode`、`pid`、`baseUrl`、可选 `mcpUrl`、`workingDir`、可选 `threadId`、`startedAt`，以及 version-2 writer scope（`repoId`/`worktreeId`/可选 `workspaceId`/`leaseFence`）；它永不包含 token、token hash、token path、provider credentials、headers 或 provider request/response bodies。

Write control 仅限本地。`--control write` 与 `--stdio` 组合会被拒绝，并要求 `--host` 是 loopback（`127.0.0.1`、`::1` 或 `localhost`）。使用相同默认路径启动第二个 write-control 实例会以 `CONTROL_INSTANCE_CONFLICT` 快速失败；只有调用方有意管理多个本地实例时，才使用不同的 `--control-token-file` 和 `--control-info-file` 路径。

Automation clients 使用 `POST /api/code/controller/attach` 连接，请求体 `{ "clientId": "...", "kind": "automation" }`，header `X-Libra-Control-Token`，然后使用返回的 `X-Code-Controller-Token` 写入。Automation-held leases 对 `/api/code/messages`、`/api/code/interactions/{id}`、`/api/code/controller/detach` 和 `/api/code/control/cancel` 同时要求两个 tokens。本地 TUI 可以用 `/control reclaim` 重新取得控制权，这会使 automation lease 失效。Code UI 写请求体上限为 256KiB。当会话广告 `capabilities.commandIdempotency`（当前仅 headless web-only）时，`POST /api/code/messages` 接受 `{ "text": "...", "commandId": "..." }` 以支持重试去重（相同 id + 相同 text 幂等；相同 id + 不同 text 返回 `COMMAND_PAYLOAD_CONFLICT`）。运行时会用活动 controller `clientId` 的 SHA-256 fence 命名空间化 `commandId`（原始 clientId 不会写入 durable command log）。`commandIdempotency` 仅在配置了 durable SessionStore command admission 时广告。其他 runtime 若收到 `commandId` 会显式拒绝，而不会静默忽略。

`GET /api/code/diagnostics` 返回为本地工具准备的脱敏 observe-only 状态摘要。Control attach、detach、submit、respond 和 cancel 操作会通过 runtime audit sink 发出 `local-tui-control/v1` audit events。Stdio automation clients 使用 [`libra code-control --stdio`](code-control.md)；`libra code --stdio` 仍是 MCP stdio server，不控制 live TUI。

### Web Browser Control

`--browser-control <off|loopback>` 控制嵌入式 UI 的基于租约的写表面是否可用。默认值感知模式：

| 入口 | 默认 `--browser-control` |
|------|--------------------------|
| TUI 会话（`libra code` 且不带 `--web-only`） | `off` |
| `libra code --web-only --provider codex` | `loopback` |
| 使用任意其他 provider 的 `libra code --web-only` | `off` |

当 `--host` 不是 loopback 地址时，选择 `loopback` 会被拒绝；该标志也与 `--stdio` 冲突。浏览器服务端 endpoints 在 `code_router()` audit matrix（`src/internal/ai/web/mod.rs`）中标记：

- `GET /api/code/session`、`GET /api/code/events`、`GET /api/code/diagnostics`、`GET /api/code/threads`、`GET /api/code/usage`、`GET /api/code/skills`、`GET /api/code/goal/status` — 仅 loopback observe。
- `POST /api/code/controller/attach` — loopback。`kind: "automation"` 请求还要求 `X-Libra-Control-Token`。handler **签发** lease 的 `controllerToken`（不期待调用方发送它）。
- `POST /api/code/controller/detach`、`POST /api/code/messages`、`POST /api/code/interactions/{id}` — loopback + `X-Code-Controller-Token`；`Automation` leases 还要求 `X-Libra-Control-Token`。
- `POST /api/code/control/cancel` — loopback + `X-Code-Controller-Token`。`Automation` leases 也要求 `X-Libra-Control-Token`；这是与 TUI `Esc` cancel 路径的唯一区别。
- `POST /api/code/task/dispatch` — loopback + `X-Code-Controller-Token`；用户发起的 sub-agent dispatch 需要 active controller write lease（browser 或 automation）。Automation lease 额外要求 `X-Libra-Control-Token`。
- `POST /api/code/goal/start`、`POST /api/code/goal/cancel` — loopback + `X-Code-Controller-Token`；goal mutation 需要 active controller lease。
- `POST /api/code/skills/activate`、`POST /api/code/session/resume` — loopback + `X-Code-Controller-Token`（位于 write router，256 KiB body limit）；两者均需要 controller write lease；Automation lease 另需 `X-Libra-Control-Token`。resume 会拒绝 busy 或 `indeterminate_side_effect` snapshot；在证明目标 thread 可加载后当前 fail-closed 为 `SESSION_RESUME_REQUIRES_RESTART`（尚无 in-process AgentRuntime swap）。skill activate 在 discoverability 校验后 fail-closed 为 `SKILL_ACTIVATION_UNSUPPORTED`，直到存在 provider 消费路径。

浏览器写请求共享与自动化控制相同的 256 KiB body limit 和 audit-sink wiring。浏览器只在内存中持久化 lease；重新加载页面会丢弃 lease，下一次写入会重新 attach。

浏览器写入（含 `kind: "browser"` 的 `POST /controller/attach`）还要求可信的 loopback `Origin`（或同源 `Referer` 回退），且必须匹配 Code UI bind 地址（精确的 `http://<bound-ip>:<port>`；绑定到经典 loopback 时额外接受 `localhost` / `127.0.0.1` / `[::1]` 别名）。缺失或跨站 Origin 以 `ORIGIN_REQUIRED` fail-closed。Automation 写入走 `X-Libra-Control-Token` / controller lease，**不得**用 Origin 代替身份校验。浏览器与 automation 生产者共用按 session 的写速率限制（`LIBRA_CODE_SESSION_WRITE_RATE_LIMIT` / `LIBRA_CODE_SESSION_WRITE_RATE_WINDOW_SECS`，默认 120 次 / 60s），超限返回 `429 RATE_LIMITED`，窗口恢复后可继续。

嵌入式 SPA 的 session-lifecycle 面板通过 `GET /api/code/threads` 列出 threads，经 `POST /api/code/control/cancel` 取消当前 turn（当 `controller.canWrite` 为 false 时 fail-closed），并经 `POST /api/code/session/resume` 以 `{ "threadId": "..." }` 发起 resume。Thread 列表按仓库存储根共享（跨 linked worktree）；列表项在 ThreadProjection 持久化 per-thread cwd 之前省略 `workingDir`。

Usage 面板镜像 W2-12 `RuntimeUsageTotals` read model（累计、本 turn 增量、sub-agent 归因），并保持 `partial`/`unknown`/`error` 可见，而不是伪装成零花费。`GET /api/code/usage` 从 durable totals 读取，失败时返回错误而不伪造零值。当 durable sub-agent 枚举不可用时，响应省略 `subAgents` 并设置 `subAgentsStatus: "unavailable"`，而不是返回空数组。

Execution/repair 面板从 live session snapshot 投影 `plans[]`、`toolCalls[]` 与 `planExecutionRepair`。Continue/Cancel 经 `POST /api/code/interactions/{id}` 发送 `selectedOption`（`continue` / `cancel`）；当投影的 `attempt >= max_attempts` 时，Continue 还会带上提高后的 `maxAttempts`（上限 10），且不在浏览器侧重新分类失败。

SSE resilience 面板展示 reconnecting / resync-required / resynced 状态，同时保留最后一次投影的 session snapshot 与 wire 提供的 cursor seq（浏览器从不自造序号）。显式 snapshot resync 走共享 store 的 `refresh()`，仅在 refresh 成功应用（或被更新的 live 更新 superseded）时报告成功；production v2 backlog/resync 事件在后续 wire 卡（W3-06/W3-08）落地，内置 SPA 切换归 W3-09。

Workflow review 面板投影 pending 的 `intent_review_choice` 与 `post_plan_choice`（network policy 同 kind，用 `metadata.phase = "networkPolicy"` 区分）。Confirm/modify/cancel（以及 execute / network-allow / network-deny / back）经 leased interaction endpoint 发送 `selectedOption`；当浏览器不能 write 时 turn cancel fail-closed。面板不保存第二套 workflow FSM，等待下一次 snapshot/SSE 更新。

当服务器绑定到非 loopback host（`--host 0.0.0.0` 或局域网地址）时，非 loopback 浏览器的 HTML navigation 会收到静态 remote access notice，而不是 SPA。该 notice 零 JavaScript，只包含 bind/remote/version/commit 占位符；asset/API fallback 返回 404，使远程 clients 无法探测 session state。Snapshot、transcript、SSE、approval 以及所有 `/api/code/*` 读写 surface 仍保持 loopback-only（`LOOPBACK_REQUIRED`）。远程人工应通过 SSH port-forward 访问 loopback（`ssh -L 3000:127.0.0.1:3000 user@host`），不要期望远端浏览器直接可写；经认证的 TLS 反代属 DEFER-04，不是本计划默认面。

默认监听端口为 `3000`。若该地址已被占用，启动会 fail-closed 并提示显式 `--port`，**不会**自动扫描下一个空闲端口。

请求 `--browser-control loopback` 且浏览器持有 active lease 时，TUI 初始 controller 是 `LocalTui`（可见 owner，可 reclaim），而不是 `Fixed { Tui }`（永久阻塞）。如果 TUI 也想驱动写入，必须同时提供 `--control write` 和 `--browser-control loopback`；两个 writer 通过同一个 `TuiControlCommand` channel 串行化。

对于 `--web-only` 非 Codex providers（`--provider ollama` 是规范 headless 验证路径），Libra 构建 [`HeadlessCodeRuntime`](../../../src/internal/ai/web/headless.rs) 生命周期宿主，并将 [`AgentRuntimeCodeUiAdapter`](../../../src/internal/ai/web/agent_runtime_adapter.rs) 挂载为生产浏览器写路径 owner。浏览器 submit 进入串行的 `AgentRuntimeWorker`：普通（非 `/` 前缀）消息走与 TUI 等价的 Phase 0 plan 工具白名单，使 direct chat 无法绕过默认 mutating gate；以 `/` 开头的消息保留显式 direct tool loop。完整 IntentSpec → Phase 1 → repair 对等仍属 GATE-WEB-PLAN。Headless 模式公布 `messageInput`、`streamingText`、`toolCalls`、`planUpdates`、`patchsets`、`interactiveApprovals`、`structuredQuestions` 和 `providerSessionResume`。`--web-only --resume <thread_id>` 会在同一工作目录加载匹配会话，并在启动浏览器服务器前应用有界的 durable Code UI projection suffix。`--resume` 在 `--stdio` 模式及 `--provider codex` 下仍不可用；managed app-server 使用独立的 session protocol。`update_plan` 投影到 `plans[]`，`apply_patch` metadata 投影到 `patchsets[]`。取消在工具的 mutation boundary 之前采用 cooperative 方式；一旦可能变异的工具已经开始，cancel 会返回可操作的错误，当前 turn 保留到得到可判定结果为止。Libra 不会 hard-abort 该副作用，也不会把它重标为普通 `cancelled` turn。

对于 `--web-only --provider codex`，managed app-server 的 websocket 通知归一到共享 runtime `AgentEvent` 信封（与其他 provider 同一 projection 路径）。未知 Codex method 走显式可诊断的 `ProviderNotification` fallback，不 silent drop、也不 panic。Ask-mode approvals 仍等待浏览器 `respond_interaction`，直到 W3-07 界定共享交互所有权。

### Code UI Wire Contract

Code UI JSON contract 使用 camelCase 字段名和 snake_case 枚举值。Rust 事实来源是 `src/internal/ai/web/code_ui.rs`；浏览器镜像是 `web/src/lib/code-ui/types.ts`；`tests/ai_code_ui_wire_test.rs` 固定 wire shape。

`GET /api/code/session` 返回 `CodeUiSessionSnapshot`：

| 字段 | 类型 | 契约 |
|------|------|------|
| `sessionId` | string | 为兼容性保留的 runtime session identifier。 |
| `threadId` | string, optional | 规范的持久化 Libra thread ID；存在时，resume、graph、Web、MCP 和 diagnostics 流程应优先使用它。 |
| `workingDir` | string | 会话工作目录。 |
| `provider` | object | `{ provider, model?, mode?, managed }`。 |
| `capabilities` | object | 八个 booleans：`messageInput`、`streamingText`、`planUpdates`、`toolCalls`、`patchsets`、`interactiveApprovals`、`structuredQuestions`、`providerSessionResume`。 |
| `controller` | object | `{ kind, ownerLabel?, canWrite, leaseExpiresAt?, reason?, loopbackOnly }`；`kind` 是 `none`、`browser`、`automation`、`tui` 或 `cli`。 |
| `status` | string | `idle`、`thinking`、`executing_tool`、`awaiting_interaction`、`completed`、`error` 或 `indeterminate_side_effect`。最后一种表示变异命令可能已生效；再次尝试前必须先完成 reconciliation。 |
| `transcript` | array | 带 `id`、`kind`、可选 `title` / `content` / `status`、`streaming`、`metadata`、`createdAt`、`updatedAt` 的条目。 |
| `plans` / `tasks` / `toolCalls` / `patchsets` | arrays | Workflow、Summary、Diff 和 Terminal panes 使用的 runtime projections。 |
| `interactions` | array | 待处理/已解决的 UI prompts。`kind` 是 `approval`、`sandbox_approval`、`request_user_input`、`intent_review_choice`、`post_plan_choice` 或 `plan_execution_repair`。待处理的 plan-repair interaction 提供 `continue` 和 `cancel`，通过正常 interaction endpoint 回应。 |
| `planExecutionRepair` | object, optional | Runtime-owned plan-execution repair 状态。它含 snake_case 的 `state`、有界且经 runtime 脱敏的 failure `evidence`（`output`、`diagnostics`、`attempt`、`max_attempts`），并在 `awaiting_user` 时含 `interaction_id`。`automatic_repair` 表示进行中的重试；`awaiting_user` 只会在配置的重试次数耗尽后出现：Code UI Continue 必须提供更高的 `maxAttempts`（例如当前上限为 2 时发送 `{ "selectedOption": "continue", "maxAttempts": 3 }`），否则返回 `PLAN_REPAIR_RETRY_LIMIT_REACHED`；也可以提供手动修订指导。Cancel 为终态。`intent_spec_revision` 和 `manual_action` 需要新的用户定向 workflow。 |
| `updatedAt` | string | ISO 8601 更新时间戳。 |

`GET /api/code/events` 流式传输 `CodeUiEventEnvelope` 记录，包含 `seq`、`type`、`at` 和 `data`。事件 `type` 是 `session_updated`、`status_changed` 或 `controller_changed`；`session_updated` 携带完整 `CodeUiSessionSnapshot`。

`GET /api/code/threads` 返回 `{ items, nextOffset? }`。每个 item 有 `id`、可选 `title`、`archived`、可选 `currentIntentId`、可选 `workingDir`、`createdAt` 和 `updatedAt`。在 ThreadProjection 持久化 per-thread cwd 之前省略 `workingDir`（不要用 server cwd 冒充 linked-worktree thread）。`limit` 默认 50 并 clamp 到 200；格式错误的 `limit` 或 `offset` 返回 `INVALID_QUERY_PARAM`。

`GET /api/code/skills?provider=<slug>&skill=<name>` 返回 curated A0-07 `{ items: [{ name, provider }] }`。未知 `provider` slug 返回 `INVALID_SKILL_PROVIDER`（与 activate 相同）；省略 `provider` 时列出全部 curated providers。`POST /api/code/skills/activate` 接受 `{ provider, name }`；在 discoverability 校验后当前返回 `SKILL_ACTIVATION_UNSUPPORTED`，直到存在 in-process provider activation 路径。

Code UI API 错误使用 `{ error: { code, message } }`：

| Code | HTTP | 含义 |
|------|------|------|
| `LOOPBACK_REQUIRED` | 403 | 非 loopback client 试图访问 API route。 |
| `PAYLOAD_TOO_LARGE` | 413 | 写请求体超过 256 KiB。 |
| `ORIGIN_REQUIRED` | 403 | 浏览器写/attach 缺少可信 loopback `Origin`（或同源 `Referer`），或提交了跨站 Origin。 |
| `RATE_LIMITED` | 429 | 当前 session 写配额耗尽；等待速率窗口恢复后重试（见 `Retry-After`）。 |
| `CONTROL_DISABLED` | 403 | 当前进程未启用 automation control。 |
| `MISSING_CONTROL_TOKEN` / `INVALID_CONTROL_TOKEN` | 403 | Automation control token 缺失或无效。 |
| `MISSING_CONTROLLER_TOKEN` / `INVALID_CONTROLLER_TOKEN` | 403 | Lease token 对写路由缺失或无效。 |
| `INVALID_CONTROLLER_KIND` | 400 | Controller attach 请求了不支持的 kind。 |
| `CONTROLLER_CONFLICT` | 409 | 另一个 live controller 拥有 lease，或会话正忙。 |
| `INTERACTION_NOT_ACTIVE` | 409 | respond 目标 interaction 没有活跃的 runtime turn。 |
| `BROWSER_CONTROL_DISABLED` | 403 | 浏览器写控制已禁用。 |
| `AUTOMATION_CONTROLLER_REQUIRED` | 403 | 用非 automation lease 调用了 automation-only 路径。 |
| `CODE_UI_UNAVAILABLE` | 404 | 没有 active `libra code` session 附加到 Web 服务器。 |
| `INVALID_QUERY_PARAM` | 400 | 查询解析失败，目前用于 `/threads` 分页。 |
| `INVALID_COMMAND_ID` | 400 | `commandId` 为空、过长，或包含空白/控制字符。 |
| `STORAGE_PATH_INVALID` / `STORAGE_ROOT_UNRESOLVED` / `STATUS_UNAVAILABLE` / `THREAD_LIST_FAILED` / `DB_UNAVAILABLE` / `USAGE_UNAVAILABLE` / `SESSION_RESUME_LOAD_FAILED` / `INTERNAL_ERROR` | 500 | 服务端 storage、status、projection、database、usage、resume load 或 fallback internal failure。 |
| `INVALID_SKILL_PROVIDER` / `SKILL_NOT_DISCOVERABLE` | 400 | skill provider 不是 A0-07 slug，或 skill 对该 provider 不可发现。 |
| `SKILL_ACTIVATION_UNSUPPORTED` | 422 | skill 可发现，但尚无 in-process activation 路径。 |
| `SESSION_RESUME_BUSY` | 409 | thinking 或 tool-running session 不能被替换。 |
| `SESSION_RESUME_NOT_FOUND` | 404 | 当前工作目录下没有匹配 session。 |
| `SESSION_RESUME_REQUIRES_RESTART` | 422 | 目标 thread 可加载，但尚无 in-process AgentRuntime swap；需用 `libra code --resume <threadId>` 重启。 |
| `RECONCILIATION_REQUIRED` | 409 | 变异 turn 需要人工 reconciliation；在检查 durable session 数据前不要自动重放。 |
| `COMMAND_PAYLOAD_CONFLICT` | 409 | 相同 `commandId` 被复用到不同的消息 payload。 |
| `COMMAND_ALREADY_TERMINAL` | 409 | 相同 `commandId` 已以 failed/cancelled/indeterminate 终态结束；重试需分配新的 `commandId`。 |
| `PLAN_REPAIR_RETRY_LIMIT_REACHED` | 409 | plan-repair Continue 请求未提高已耗尽的自动重试上限。使用更高的 `maxAttempts` 重试（例如当前上限为 2 时 `{ "selectedOption": "continue", "maxAttempts": 3 }`）、提供手动修订指导，或取消 repair。 |
| `UNSUPPORTED_OPERATION` | 422 | Runtime 拒绝尚不支持的请求操作。 |

### Web Search

`web_search` 工具要求会话网络策略允许 outbound access。如果 `BRAVE_SEARCH_API_KEY` 可从 `vault.env.BRAVE_SEARCH_API_KEY` 或进程环境获得，Libra 会先尝试 Brave Search API，并返回结果标题、URL 和 snippets。如果 Brave 未配置或请求失败，Libra 会回退到零配置 DuckDuckGo HTML endpoint。

### Approval Policies

| 值 | 别名 | 说明 |
|----|------|------|
| `never` | -- | 不提示；危险命令直接拒绝。 |
| `allow-all` | `allow_all`、`always`、`accept` | 不提示；本会话允许每一条命令（`allows_all_commands`）。 |
| `on-failure` | `on-failure` | 仅在沙箱拒绝后重试时提示。 |
| `on-request` | `on-request` | 默认在沙箱内运行；当升级或策略需要时提示（默认）。 |
| `untrusted` | `unless-trusted`、`untrusted` | 对非 trusted 操作提示；已知安全读取自动允许。 |

### Context Modes

| 值 | 别名 | 说明 |
|----|------|------|
| `dev` | `development` | 常规开发工作流。 |
| `review` | `code-review` | 聚焦代码审查。 |
| `research` | `explore` | 探索式研究和分析。 |

## 常用命令

```bash
# 使用默认 Gemini provider 启动 TUI 会话
libra code

# 使用 Anthropic Claude 启动
libra code --provider anthropic --model claude-sonnet-4-20250514

# 只启动 Web 并绑定所有接口；远程浏览器会看到 loopback-only notice
libra code --web-only --port 8080 --host 0.0.0.0

# 远程人工应 SSH port-forward 到已绑定的端口，而不是直接开放写面
# ssh -L 8080:127.0.0.1:8080 user@host
# 然后在本地浏览 http://127.0.0.1:8080

# 浏览器驱动的本地 Ollama 会话
libra code --web-only --provider ollama --port 4400

# 允许浏览器通过 loopback 写控制（Codex web-only 默认是 loopback）
libra code --web-only --provider codex --browser-control loopback

# 启用本地自动化写控制（写入 token + lease discovery 文件）
libra code --control write

# 从 dotenv 风格文件加载 provider keys（覆盖陈旧 shell env vars）
libra code --env-file .env.test

# 为 Claude Desktop 集成通过 stdio 运行 MCP
libra code --stdio

# 使用启用 reasoning 的 DeepSeek
libra code --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true
libra code --env-file .env.test --provider deepseek --model deepseek-v4-pro --deepseek-thinking enabled --deepseek-reasoning-effort high --deepseek-stream true

# 使用 Kimi（Moonshot AI）和 K2.6 默认值；为了降低延迟可关闭 thinking
libra code --provider kimi
libra code --provider kimi --model kimi-k2-thinking --kimi-thinking enabled
libra code --provider kimi --model kimi-k2.6 --kimi-thinking disabled

# 使用本地 Ollama 模型；普通请求会先生成可审阅计划
libra code --provider ollama --model llama3 --api-base http://127.0.0.1:11434/v1

# 为远程/云 Ollama endpoint 使用紧凑 tool schemas
libra code --provider ollama --model minimax-m2.7:cloud --api-base http://192.168.0.5:11434/v1 --ollama-compact-tools

# 为一次 Ollama 运行启用 high thinking
libra code --provider ollama --model qwen3.6 --ollama-thinking high

# 使用本地 Ollama 模型时捕获 provider/TUI diagnostics
LIBRA_LOG='libra::internal::ai=debug,libra::internal::tui=debug' \
LIBRA_LOG_FILE=/tmp/libra-code.log \
libra code --repo=/Volumes/Data/linked --provider ollama --model gemma4:31b

# 在 TUI 或非 Codex headless Web server 中恢复规范 Libra 线程
libra code --resume 11111111-1111-4111-8111-111111111111
libra code --web-only --provider ollama --resume 11111111-1111-4111-8111-111111111111

# 检查同一线程的版本图
libra graph 11111111-1111-4111-8111-111111111111

# 从该仓库外部检查线程图
libra graph 11111111-1111-4111-8111-111111111111 --repo /Volumes/Data/linked

# 以 code review 上下文和严格 approval 启动
libra code --context review --approval-policy untrusted

# 使用 Codex 的先规划后执行模式
libra code --provider codex --plan-mode
```

## 人工输出

输出会根据模式通过 TUI、Web 界面或 MCP 协议交付。默认 TUI 模式没有面向行的 stdout。在 generic provider TUI 中，普通纯文本请求会自动启动 plan workflow；显式 slash commands 保持其命令专用行为。Generic provider planning 使用两步审阅：LLM 首先起草 IntentSpec 供确认，然后确认后的 IntentSpec 会被送回 LLM，用于在任何执行开始前生成可审阅执行计划。如果已确认计划执行失败，或 orchestrator 在到达最终决策前中止，Libra 会将失败证据反馈给 planner，要求其添加或调整修复步骤，并在自动修复阈值内自动运行修订计划。达到阈值后，TUI 会等待开发者使用更高的重试限制继续（例如 `/plan continue <higher-limit>`），或提供显式计划修复指导；普通 `continue` 会保留已耗尽的限制并返回 `PLAN_REPAIR_RETRY_LIMIT_REACHED`。Cancel 为终态。Web 服务器提供嵌入式 Next.js 应用。Stdio 模式通过遵循 Model Context Protocol 的 JSON-RPC 消息通信。

## Diagnostics

`libra code` 支持通过 `RUST_LOG` 或 `LIBRA_LOG` tracing；两者都设置时，`LIBRA_LOG` 优先。对于 TUI 会话，推荐使用 `LIBRA_LOG_FILE=<path>`，这样 diagnostics 会写入普通日志文件，而不是 alternate-screen terminal。当设置 `LIBRA_LOG_FILE` 但没有显式 log filter 时，Libra 默认使用 `libra=debug`。

对 Ollama provider 失败，有用的 diagnostics 是：

```bash
mkdir -p /tmp/libra-logs
LIBRA_LOG='libra::internal::ai=debug,libra::internal::tui=debug' \
LIBRA_LOG_FILE=/tmp/libra-logs/libra-code-ollama.log \
libra code --repo=/Volumes/Data/linked --provider ollama --model gemma4:31b
```

如果 TUI 报告 Ollama 503，也捕获本地 server 状态：

```bash
ollama ps >> /tmp/libra-logs/libra-code-ollama.log
ollama list >> /tmp/libra-logs/libra-code-ollama.log
```

## 设计动机

### 为什么采用 TUI + Web server 混合？

TUI 为已经在终端中工作的开发者提供低延迟、键盘驱动界面。后台 Web 服务器同时运行，使同一会话可以从浏览器访问，这有助于与队友共享上下文、查看丰富 diff，或从移动设备访问会话。`--web-only` 标志会完全去掉 TUI，用于没有可用终端的 headless 或远程服务器部署。

### 为什么支持多个 AI provider？

不同 provider 擅长不同任务，并具有不同成本/延迟画像。Gemini 因慷慨的免费层和快速响应而作为默认值。Anthropic Claude 擅长谨慎 reasoning 和代码审查。本地 Ollama 支持完全离线开发。通过抽象在 `CompletionClient` trait 后面，添加新 provider 只需要实现该 trait，无需触碰 session、tool 或 TUI 层。

### 为什么集成 MCP？

Model Context Protocol（MCP）是连接 AI clients 与 tool servers 的开放标准。通过支持 `--stdio` 模式，Libra 可以作为任意 MCP 兼容 client（例如 Claude Desktop）的 tool server。Libra 暴露 allowlisted `run_libra_vcs` tool，用于 `status`、`diff`、`branch`、`log`、`show`、`show-ref`、`ls-files`、`add`、`commit` 和 `switch` 等版本控制操作，因此外部 AI agents 直接使用 Libra，而不是调用 Git。`run_libra_vcs` 只接受这些 Libra 子命令；它不是 Git 兼容 shell。检查仓库状态时，优先使用 `status --json` 或 `status --porcelain v2 --untracked-files=all`，并使用 `ls-files` 检查 tracked 与 untracked 仓库路径（例如 `ls-files --others --exclude-standard` 列出忽略感知的 untracked 文件）。Libra-managed execution 也会拒绝直接的 `git` shell 命令。

### 为什么需要 approval policies？

AI agents 在开发者机器上执行 shell 命令存在真实安全风险。五层 approval 系统在效率和控制之间取得平衡：
- `never` 用于完全锁定环境，agent 只能读取。
- `allow-all` 是另一个极端：不提示且每条命令都运行，适用于摩擦大于风险的可信一次性或沙箱环境。
- `on-failure` 允许 agent 尝试沙箱执行，只有失败时才询问。
- `on-request`（默认）把所有操作放进沙箱，并在 agent 或沙箱策略需要时升级。
- `untrusted` 是最保守的交互模式，对已知安全读取之外的任何操作都提示。

### 为什么持久化和恢复会话？

长编码会话会积累大量上下文：文件编辑、对话历史、工具输出。意外关闭终端后丢失这些上下文很痛苦。Session persistence 会存储完整 conversation 和 tool state，而 `--resume <thread_id>` 会恢复规范 Libra 线程。

嵌入式 Code UI 在其 session snapshot 中以 `threadId` 暴露相同规范标识。较旧的 `session_id` 字段仍保留以维持兼容，但新集成应使用 `threadId` 作为 resume、Web、MCP 和 diagnostics 流程的 key。

对于持久化的非 Codex Web session，初始 session 写入是启动 turn 的前提：写入失败时 Libra 不会启动 turn，浏览器可修复存储后重试。若后续持久化失败，live session 会变为 `indeterminate_side_effect`，并阻止新的 submit 或 interaction reply；重启或 reconciliation 前必须检查 durable session data。

收到 `Ctrl-C` / `SIGINT` 或 `SIGTERM` 时，非 Codex headless / web-only 进程会关闭浏览器命令 admission，再走统一的进程级 lifecycle shutdown owner（runtime / listeners / managed child / control），并共享同一 deadline。read-only/model 工作会 cooperative cancel；已经开始的 mutating tool 在预算内被允许完成。若超过期限，`libra code` 会以明确的 shutdown failure 退出，重启前必须检查 session 并完成 reconciliation。进程编排应优先发送 `SIGTERM`（或 `Ctrl-C`/`SIGINT`），避免直接 `SIGKILL`，以便端口、lease 与子进程被干净释放。

## 参数对比：Libra vs Git vs jj

| 参数 | Libra | Git | jj |
|------|-------|-----|----|
| 交互式 AI 会话 | `libra code` | 不可用 | 不可用 |
| TUI 模式 | 默认 | 不可用 | 不可用 |
| Web 模式 | `--web-only` | 不可用 | 不可用 |
| MCP/stdio 模式 | `--stdio` | 不可用 | 不可用 |
| AI provider 选择 | `--provider` | 不可用 | 不可用 |
| 会话恢复 | `--resume <thread_id>`（TUI 及非 Codex `--web-only`） | 不可用 | 不可用 |
| 工具 approval policy | `--approval-policy` | 不可用 | 不可用 |

注意：Git 和 jj 都没有 `libra code` 的等价物。该命令体现了 Libra 作为 AI-agent-native 版本控制系统的核心差异。Git 生态中最接近的类似物是 GitHub Copilot CLI 或 aider 等第三方工具，它们是独立应用，而不是集成 VCS 命令。

## 错误处理

| 场景 | 行为 | 退出 |
|------|------|------|
| 同时指定 `--web-only` 和 `--stdio` | Clap 参数冲突错误 | non-zero |
| 选中 provider 缺少 API key | 带 provider 名称和期望 env var 的 fatal error | non-zero |
| 端口已被占用 | fatal：指出 `host:port`，并要求显式 `--port`（不自动扫描） | non-zero |
| TUI 模式下没有可用终端 | 回退或报告错误 | non-zero |
| 恢复时找不到 Thread ID | 带规范 `thread_id` 的 fatal error | non-zero |
| `--control write --stdio` | 用法错误；MCP stdio 和本地 TUI automation stdio 是不同模式 | non-zero |
| `--control write --host 0.0.0.0` 或其他非 loopback host | 用法错误；write control 仅限 loopback | non-zero |
| 另一个 live `--control write` 拥有相同 control lock | 可用时带已有 PID/URL 的 `CONTROL_INSTANCE_CONFLICT` | non-zero |
| Control token file 是 symlink、非普通文件，或在 Unix/macOS 上不是 `0600` | Web 服务器启动前 fatal setup error | non-zero |
