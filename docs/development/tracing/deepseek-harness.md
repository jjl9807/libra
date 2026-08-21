# DeepSeek Harness 插件集成设计

## 1. 设计结论

Libra 不应在 DeepSeek Harness 中再实现一个 Agent loop，而应作为 Harness 的**版本化记忆、工作区治理和工程 provenance 层**。

DeepSeek Harness 的公开定位是“一切皆插件”：模型、工具、技能、会话、沙箱、存储、循环、调度和 UI 都由插件组合。Harness 负责让 Agent 工作，Libra 负责让 Agent 的工作可追溯、可恢复、可验证和可协作。

参考资料：

- [DeepSeek Harness 产品页](https://deepseek.com/harness/)
- [DeepSeek Harness 架构文档](https://github.com/deepseek-ai/deepseek-harness/blob/99f6f02fecdb7dff40c3fbc9470f5907c29f74ca/docs/architecture.md)
- [Libra Agent 命令入口](../../../src/command/agent/mod.rs)
- [Libra Agent hook 适配器](../../../src/command/agent/hooks.rs)
- [Libra Code 控制协议](../../../docs/commands/code.md)

本文档以本地 DeepSeek Harness checkout 的 `dsh-v0.1.0-rc.7`
（commit `99f6f02fecdb7dff40c3fbc9470f5907c29f74ca`）为事实基线；事件名、
`sessionPersistence`、`agent.inject()` 和 `--dump-config` 均以该版本为准。
本文档中标为“目标态”“提议契约”或“待实现”的接口不是当前已存在的 Libra API。

### 1.1 事实源边界

两个系统的事实源必须明确分工：

| 领域 | 权威事实源 |
| --- | --- |
| Agent 当前上下文、消息历史、模型回放 | DeepSeek Harness session log |
| 代码、分支、索引、worktree | Libra repository/VCS |
| intent、plan、checkpoint、evidence、decision | Libra AI 对象与审计记录 |
| Agent 运行轨迹的跨系统投影 | Libra provenance overlay |

第一版不要让 Libra 直接替换 Harness 的 `sessionPersistence`。两边的事件模型和 replay 约束不同，直接共用存储会把 Harness 的 compaction、fork、UI replay 与 Libra 的对象生命周期耦合在一起。应先做“事件投影 + provenance overlay”。

## 2. 目标架构

```mermaid
flowchart LR
    DSH[DeepSeek Harness]
    TOOLS[@libra/dsh-tools]
    SESSION[@libra/dsh-session]
    WORKSPACE[@libra/dsh-workspace]
    CONTEXT[@libra/dsh-context]
    UI[@libra/dsh-ui]
    BUNDLE[@libra/dsh-bundle]
    BRIDGE[libra agent bridge --stdio<br/>JSON-RPC NDJSON]
    LIBRA[Libra Rust Runtime]

    DSH --> TOOLS
    DSH --> SESSION
    DSH --> WORKSPACE
    DSH --> CONTEXT
    DSH --> UI
    DSH --> BUNDLE
    TOOLS --> BRIDGE
    SESSION --> BRIDGE
    WORKSPACE --> BRIDGE
    CONTEXT --> BRIDGE
    UI --> BRIDGE
    BUNDLE --> BRIDGE
    BRIDGE --> LIBRA
```

标准写入入口是目标态的 `libra agent bridge --stdio`。Harness
通过 JSON-RPC NDJSON 将 session、workspace、checkpoint、evidence 和 provenance
操作发送给 Libra 的 Agent ingress/runtime。

建议将集成拆成多个可独立启停的 npm 包，而不是一个不可拆分的“大插件”：

| 包 | 责任 |
| --- | --- |
| `@libra/dsh-tools` | 模型可见的高层 VCS、checkpoint、历史和 review 工具 |
| `@libra/dsh-session` | Harness session/event 到 Libra 的批量投影、outbox 和断点续传 |
| `@libra/dsh-workspace` | session/subagent 与 Libra worktree、workspace lease 的绑定 |
| `@libra/dsh-context` | Libra skill、历史摘要、decision 和 evidence 的按需上下文 |
| `@libra/dsh-ui` | checkpoint、diff、commit、evidence 的 Harness UI 卡片 |
| `@libra/dsh-bundle` | 一份可直接启用的 Harness profile/bundle |

## 3. 第一阶段：Agent Bridge 能力插件

Harness 不直接访问 Libra 数据库，也不再以旧的工具服务器传输作为标准写入路径。目标态由
`@libra/dsh-bundle` 启动一个受 Libra 管理的 JSON-RPC NDJSON bridge：

```yaml
- id: libra
  name: '@libra/dsh-bundle'
  config:
    bridge:
      protocol: jsonrpc-ndjson
      command: libra
      args: ['agent', 'bridge', '--stdio']
      cwd: /path/to/repository
```

上面的 bundle 配置和 `libra agent bridge --stdio` 都是目标态契约，当前 checkout
还没有这个子命令。当前 `libra agent` 主要是外部 Agent capture 的操作面；实现 bridge
时应复用其内部 session/checkpoint/provenance service，而不是把写入逻辑放进 Node 插件。

入口职责必须保持分离：

| 入口 | 职责 |
| --- | --- |
| `libra agent bridge --stdio` | Harness 写入 session、event、workspace、checkpoint、evidence 和 provenance；标准入站协议 |
| `libra code --control stdio` | 控制 Libra 自己运行中的 Agent session；不是 Harness 的持久化入口 |
| `libra agent hooks ...` | Claude/Codex 等外部 Agent 的兼容 hook 适配器，转发到统一 ingress |
| `libra agent import ...` | 历史 transcript 的显式导入适配器 |
| `libra agent rpc ...` | 启动受信任的外部 RPC 工具；不是 Harness 入站接口 |

### 3.1 Bridge 方法与模型可见工具

Harness 插件可以将模型可见工具映射为 bridge 方法，但模型不应直接看到低层
存储操作。第一版建议暴露以下高层、稳定、带风险语义的能力：

- `libra_context`：当前仓库、分支、worktree、活动任务和最近 checkpoint。
- `libra_status`：结构化工作区状态。
- `libra_diff`：工作区、暂存区或指定 checkpoint 的差异。
- `libra_history_search`：搜索历史 session、intent、decision 和 evidence。
- `libra_checkpoint`：创建、列出和查看 checkpoint。
- `libra_commit`：将当前代码与 session、intent、checkpoint、evidence 关联后提交。
- `libra_review`：针对指定 checkpoint 执行只读 review。
- `libra_restore_checkpoint`：恢复前必须经过人工确认。

对应 bridge 方法可以按以下命名分组：

```text
context.get
status.get / diff.get / history.search
workspace.claim / workspace.release
session.open / session.flush / session.close
checkpoint.create / checkpoint.list / checkpoint.restore
checkpoint.show
evidence.append / provenance.append
commit.create
review.run
```

`create_intent`、`create_task`、`create_run` 等低层对象操作只作为 Rust ingress
内部 service 使用，不应直接暴露给模型。否则会增加 schema token、工具组合错误和错误恢复复杂度。

### 3.2 工具结果约定

每个工具都应返回结构化结果，而不是要求模型解析 CLI 文本：

```json
{
  "schema_version": 1,
  "repository_id": "...",
  "workspace_id": "...",
  "operation_id": "...",
  "status": "ok",
  "data": {},
  "warnings": []
}
```

错误至少应包含稳定的 `code`、人类可读的 `message`、受影响的资源和可采取的下一步。模型不得通过拼接 shell 字符串调用 Libra；路径、revision、tool 参数和操作类型均须在 Rust 侧校验。

## 4. 第二阶段：session 轨迹桥接

`@libra/dsh-session` 监听 Harness 的生命周期事件：

- `session/created`
- `session/event`
- `session/flush`
- `session/disposed`
- `agent/created` / `agent/disposed`
- `subagent/start` / `subagent/end`
- `session/event` 中 `event.type === 'tool/result'` 的持久事件（不是独立的 Cordis dispatch 事件）

投影事件的最小形态如下：

```json
{
  "source": "deepseek-harness",
  "session_id": "...",
  "parent_session_id": "...",
  "workspace_id": "...",
  "event_seq": 37,
  "event_type": "tool/result",
  "payload_sha256": "...",
  "payload": {}
}
```

### 4.1 可靠性要求

- 以 `(session_id, event_seq)` 做幂等键，重复提交不得生成重复对象。
- 事件使用批量提交，避免每个 token 或工具事件单独启动进程。
- 插件维护有界本地 outbox；Libra 暂时不可用不能静默丢失轨迹。
- `session/flush` 时等待已接受的批次完成；普通事件路径不应阻塞 Agent loop。
- 进程重启后从 Libra 返回的**提议 bridge API 字段** `last_acked_seq` 继续同步；它不是当前 Libra 已有的返回字段，必须在 bridge schema 中固定并测试。
- 事件 payload 保存哈希和来源，不依赖时间戳推断顺序。
- 子 Agent 使用独立的 `session_id`，通过 `parent_session_id` 和 delegation depth 建立谱系。
- `session/disposed` 时执行有界 drain；超时必须留下可诊断状态。

所有模型工具和 session 事件都通过同一个 JSON-RPC/stdio bridge 进入 Libra；低频请求
与高频批量事件可以使用不同的方法和队列优先级，但不再维护两套协议或两套写入实现。

### 4.2 隐私和数据分级

默认保存结构化工具调用、结果、文件变更、验证结果和必要的消息摘要。原始 reasoning、完整 prompt、环境变量和敏感文件内容必须受配置控制，并在写入前经过脱敏。

未脱敏 transcript 只能通过显式授权读取或导出；每次 raw export 都应写入 Libra append-only audit log。脱敏失败时 fail-closed，不得以原文回退。

## 5. 第三阶段：workspace 与并行 Agent

Libra 的 worktree registry、workspace record 和 lease 适合为 Harness 的 session/subagent 提供工程工作区治理，但必须避免两套系统同时成为 lease 权威。

建议由 Libra 负责 repository/worktree 的所有权和 lease，Harness 的 workspace registry 只保存镜像和 UI 需要的关联。

支持三种配置模式：

```text
reuse-current       默认，兼容现有 Harness 行为
linked-per-session  每个 session 独立 linked worktree
linked-per-subagent 每个并行 subagent 独立工作区
```

约束：

- 插件不得静默切换用户当前工作区。
- 创建、切换、释放 worktree 必须有明确的 session scope。
- lease 过期、进程崩溃和 workspace 身份不一致时 fail-closed。
- 并行 Agent 不应仅依赖 Libra linked worktree；linked worktree 共享 common storage（SQLite 数据库和对象）以及分支 refs，但 HEAD 已按 worktree 作用域隔离，因此真正的分支隔离仍应使用独立 clone 或显式 branch scope。参见 [worktree scope 实现](../../../src/internal/worktree_scope.rs)。
- 子 Agent 完成后产出 checkpoint/patchset，由父 Agent 选择合并，而不是直接改写父工作区。

## 6. 第四阶段：工程治理和可验证结果

Libra 集成的核心价值不是“多几个 Git 工具”，而是把 Harness 的最终文本回答变成可审计的工程结果：

1. 在 turn 结束或用户显式请求时创建 checkpoint。
2. 将测试、lint、build 和 review 结果记录为 Evidence。
3. 将 checkpoint 与 commit、intent 和 decision 建立关联。
4. 在 `agent/turn-stopping` 阶段执行可配置的质量门禁。
5. 失败时向下一轮注入结构化反馈，而不是只注入一段非结构化文本。
6. 支持从 checkpoint fork 新 Agent 或重新运行验证。

推荐的 commit 关联字段：

```text
session_id
parent_session_id
intent_id
checkpoint_id
evidence_ids[]
agent_id
provider/model
workspace_id
```

这些字段应进入 Libra 的 provenance/decision 数据，而不是依赖 commit message 中不稳定的约定文本。

## 7. 权限与风险策略

权限应同时经过 Harness 和 Libra 两层：

| 操作 | 默认策略 |
| --- | --- |
| status、diff、log、history、checkpoint list | 自动允许 |
| add、commit、checkpoint create、worktree create | Harness approval |
| branch switch、restore checkpoint | 明确确认 |
| reset、clean、push、publish、删除 worktree | 默认拒绝，显式授权 |

当前低层 AI object adapter 存在共享默认 actor 和调用方自报 `actor_kind`/`actor_id` 的兼容行为。新的 DSH bridge 不能沿用这个共享默认身份，也不能把调用方自报字段当作认证。可信 bridge 应在连接建立时校验 session/workspace 身份，并由 bridge 强制注入以下 actor 映射：

```text
deepseek-harness:<session_id>
deepseek-harness:<agent_id>
```

所有写操作都应具备：

- 当前 repository/worktree 身份校验。
- actor 必须绑定到已认证的 bridge session/subagent；模型可见工具不得暴露或接受 `actor_kind`/`actor_id` 作为身份凭据，Rust 侧应拒绝冲突值或覆盖它们。
- operation id 和幂等键。
- 预检后的 HEAD/index/worktree fence。
- 失败时可解释的稳定错误码。
- 审批、执行和结果的审计链。

## 8. Harness profile 适配

Libra 能力应通过 Harness profile 的 bundle 组合选择，而不是所有 profile 都默认加载。
当前 checkout 随附的 profile 是 `web` 和 `headless`；其他 profile 必须先通过
`dsh plugin --profile <name> ...` 创建并安装插件。建议新增一个名为 `libra` 的自定义
profile，而不是假设 Harness 已经内置 Libra 专用 profile。

| Harness profile/运行时表面 | Libra 默认能力 |
| --- | --- |
| `web` | status、diff、history、checkpoint、session capture |
| `headless` | status、diff、checkpoint、evidence capture；结束时输出可审计结果摘要 |
| 自定义 `libra` profile | 完整的 `@libra/dsh-bundle`，包括 session bridge、workspace lease 和 UI/context 适配 |
| Code Mode（`ctx.codeRuntime`，与 profile 正交） | 提供 `libra` SDK facade，允许在程序中组合查询和批量记录 |

历史和决策上下文必须按需注入，不能将完整 transcript 每轮塞入 system prompt。所有进入模型的 Libra context 都必须通过 Harness 的 `agent.inject()` 或正式 prompt section 写入 session log，并受 token 预算和 compaction 约束。

## 9. 建议实现顺序

### MVP

1. 独立的 `libra agent bridge --stdio` 入口；若新增顶层 Libra 命令或 `agent` 子命令，必须同时更新 `COMPATIBILITY.md` 命令行、对应 `docs/commands/<cmd>.md`（含 `Examples`）、CLI `*_EXAMPLES`/root command groups，以及相关 compat guards；新增稳定错误码还要同步 `docs/error-codes.md`。
2. `libra_context`、`libra_status`、`libra_diff`、`libra_history_search`、`libra_checkpoint`。
3. `session/event` 的批量投影、outbox、幂等和断点续传。
4. `libra_commit` / `commit.create` 与 Harness session/checkpoint/evidence 关联。
5. 双层 approval；危险操作默认关闭。

### 第二个版本

1. per-session/per-subagent workspace lease。
2. compaction-aware context anchor。
3. review/evidence 自动记录。
4. checkpoint、diff、commit、evidence 的 UI card。
5. session fork、checkpoint restore 和验证重跑。

### 后续探索

1. Libra automation 驱动 Harness headless session。
2. 跨仓库历史和 skill 检索。
3. cloud backup/publish 的显式 opt-in 集成。
4. 更完整的 Harness session persistence adapter。

## 10. 验收标准

### 协议与数据

- 先通过 `dsh plugin --profile libra add <libra-bundle-spec>` 创建并安装自定义 profile；随后
  `dsh --profile libra --dump-config` 可输出完整、可重放的插件组合。
- JSON-RPC bridge stdout 只包含协议帧，日志全部写 stderr 或 Harness logger。
- 同一事件重试不会生成重复 session、checkpoint 或 evidence。
- 杀掉任一进程后重启，事件可从最后确认序号继续同步。
- Libra 不可用时，失败状态可见且不会静默丢失数据。

### 安全与权限

- 模型无法绕过 typed tool 直接构造 shell 命令访问 Libra。
- 路径、cwd、repository identity、worktree identity 和 symlink 均 fail-closed 校验。
- commit、restore、push、publish 和删除操作均经过审批策略。
- secrets、token、PII 和 raw reasoning 按策略脱敏；脱敏失败不回退原文。

### Agent 行为

- session、subagent、workspace、checkpoint 和 commit 的关联可查询。
- 从 checkpoint 恢复后，Harness 能继续工作且不会重复消费历史事件。
- 并行 subagent 的结果不会越过 parent scope 直接覆盖父工作区。
- context 注入受 token 预算约束，且在 Harness session log 中可回放。

### 测试

- JSON-RPC bridge schema 和错误码契约测试。
- 新增或修改顶层命令的兼容性守卫：`COMPATIBILITY.md` 与 `tests/compat/matrix_alignment.rs`、`*_EXAMPLES` 与 `tests/compat/help_examples_banner`、命令文档 Examples、root command groups，以及 `tests/compat/error_codes_doc_sync.rs`（仅在引入新错误码时）。
- outbox 重试、重复提交、断线恢复和进程崩溃测试。
- worktree lease 竞争、过期和身份冲突测试。
- approval、路径穿越、symlink、敏感信息脱敏和 raw export 测试。
- 单 Agent、嵌套 subagent、fork、compaction 和恢复的端到端测试。

## 11. 非目标

第一阶段明确不做：

- 在 Libra 内重新实现 DeepSeek Harness Agent loop。
- 直接把 Libra 数据库暴露给 Node 插件。
- 用 Libra 替换 Harness 原生 session persistence。
- 默认启用 push、publish、reset、clean 或删除 worktree。
- 把所有低层 AI 对象 CRUD 暴露给模型。
- 将完整 transcript 或 reasoning 永久注入每轮上下文。

这条边界能保持两个项目各自的优势：Harness 保持插件化 Agent runtime，Libra 专注于 AI 原生版本控制、工程证据和可恢复的协作历史。
