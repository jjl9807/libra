# `libra code-control`（已删除）

> **Breaking change（W5-01）：** 弃用的 `libra code-control` 转发 shim 已在
> W5 breaking 发布中被**物理删除**。二进制现在把 `code-control` 作为未知命令拒绝
> （`libra: 'code-control' is not a libra command.`，退出码 129）。本页仅作为
> 迁移说明保留，不再描述一个可用命令。

## 迁移

请使用 canonical stdio automation client。它与被删除的 shim 使用相同的
换行分隔 JSON-RPC 2.0 协议，并默认从 `.libra/code/control.json` discovery
endpoint：

| 已删除的调用 | 替代调用 |
|---|---|
| `libra code-control --stdio --url <baseUrl> --token-file <path>`（shim 要求两个 flag 同时提供） | `libra code --control stdio --control-url <baseUrl> --control-token-file <path>` |
| `libra code-control --stdio --url $(jq -r .baseUrl .libra/code/control.json) --token-file .libra/code/control-token` | `libra code --control stdio`（默认 discovery `.libra/code/control.json`；可用 `--control-url` / `--control-token-file` / `--control-info-file` 覆盖） |

JSON-RPC 方法（`controller.attach`、`message.submit`、`events.subscribe`、
`diagnostics.get` 等）与 JSON-RPC 错误映射不变，见
[`code.md`](code.md) 的「本地自动化控制」一节。请勿把 `--control stdio` 与
弃用的 MCP-only `libra code --stdio` 传输混同（tools/resources；独立的
`libra mcp --stdio` 计划在 W5 之后，DEFER-02）。

## 示例

```bash
# 从 .libra/code/control.json discovery endpoint/token（推荐）
libra code --control stdio

# 显式 endpoint/token（替代已删除的 --url/--token-file 写法）
libra code --control stdio \
  --control-url http://127.0.0.1:3000 \
  --control-token-file .libra/code/control-token
```
