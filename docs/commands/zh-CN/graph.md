# `libra graph`

检查 Libra Code 线程版本图。

## 概要

```bash
libra --json graph <THREAD_ID> [--repo <PATH>]
libra --machine graph <THREAD_ID> [--repo <PATH>]
```

## 说明

当前 thread/version graph 在 Web Code UI（`libra code`）中打开。`libra graph` 仍读取 `.libra/` 下的 AI 投影表和正式 AI 历史。

**Breaking change（W5-08）：** 交互式 graph TUI 入口已在 W5 breaking 发布中删除。裸 `libra graph <THREAD_ID>` 现在以 usage error 加迁移提示失败；交互图视图请在 Web Code UI（`libra code`）中打开，或使用 `libra graph --json` / `--machine` 获取保持不变的结构化输出。

版本链节点包括：

- Intent 修订
- 执行计划
- 任务
- 运行
- PatchSet

当对应的投影数据可用时，图会高亮当前/最新 intent 头、选中的 plan 头、活动 task/run、最新 run 以及最新 patchset。

使用全局 `--json`（或 `--machine`）时，`libra graph` 输出结构化 JSON——面向 agent 的路径。`data` 除线程元数据（`thread_id`、`title`、`freshness`、`thread_version`、`scheduler_version`、`updated_at`，以及 `selected_plan_id` / `active_task_id` / `active_run_id`）外，还包含从最新匹配会话 workflow 日志 fold 出的可选 Code UI 覆盖字段：

| 字段 | 类型 | 含义 |
|------|------|------|
| `code_ui_status` | string 或 null | 公开 wire 的 `snake_case` Code UI 状态（`idle`、`thinking`、`executing_tool`、`awaiting_interaction`、`completed`、`error`、`indeterminate_side_effect`）。无线程会话时为 null。 |
| `code_ui_transcript_len` | number | fold 后的 transcript 条目数（无覆盖时为 0）。 |
| `code_ui_pending_interactions` | number | fold 后仍为 pending 的交互数（无覆盖时为 0）。 |

缺少会话是非致命的（覆盖字段保持 null/0）；会话 workflow 日志不可读或畸形则为硬错误，避免隐藏需要 reconciliation 的 indeterminate 栅栏。首次覆盖查找时可能在 `.libra/.../sessions/.thread_index/` 下执行一次性 thread→session 索引重建（`libra code` 启动时也会执行）；任一既有会话无法加载或索引时迁移失败封闭。无活 owner 的 Pending 变更命令在运行时重启写栅栏之前也会显示为 `indeterminate_side_effect`。

示例——以结构化 JSON 查看线程的版本图（紧凑形式用 `--machine`）：

```bash
libra graph --json 11111111-1111-4111-8111-111111111111
```

裸交互入口已在 W5 breaking 发布中删除（W5-08），以 usage error 加迁移提示失败；退出时打印该后续命令的遗留 `libra code` TUI 会话已随 TUI 启动路径一并删除（W5-06）。交互视图请在 Web Code UI 中打开该线程的图。

## 参数

| 参数 | 必需 | 说明 |
|----------|----------|-------------|
| `<THREAD_ID>` | yes | 要检查的规范 Libra 线程 UUID。 |

## 选项

| 选项 | 说明 |
|--------|-------------|
| `--repo <PATH>` | 检查指定 Libra 仓库，而不是从当前目录发现仓库。 |

## 常用命令

```bash
# 以结构化 JSON 输出某个线程的版本图
libra graph --json 11111111-1111-4111-8111-111111111111

# 输出另一个工作树中某个线程的图
libra graph --json 11111111-1111-4111-8111-111111111111 --repo /path/to/repo

# 检查后在 Code 中继续同一线程
libra code --resume 11111111-1111-4111-8111-111111111111
```

## 输出

`libra graph` 需要全局 `--json`（或 `--machine`）flag：不带该 flag 时命令以 usage error 加迁移提示退出（交互式 TUI 入口已在 W5 breaking 发布中删除；实时图视图在 Web Code UI 中）。带 `--json`/`--machine` 时把图作为单个结构化 JSON 文档写入 stdout（见 *说明* 中的 envelope），可在 agent/自动化上下文中无头运行。如果线程 ID 不是 UUID，命令会以用法错误退出；如果当前目录和 `--repo` 都无法解析为 Libra 仓库，或者找不到请求的线程，则会以仓库/投影错误退出。

## 设计说明

该图使用 Libra 的投影读模型，而不是直接解析 TUI 会话 JSON。这使视图与提供商无关：只要存在正式 AI 历史，通用 LLM 会话和托管 Codex 会话都可以被检查。

该命令接受规范 Libra 线程 ID，而不是提供商特定的会话 ID。当 `libra code` 能从会话元数据、Code UI 投影或仓库中的最新正式 AI 历史推导出线程 ID 时，会在退出后打印规范命令。
