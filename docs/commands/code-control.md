# `libra code-control`

> **Migration (W4-02):** prefer the canonical client
> `libra code --control stdio --control-url <URL> --control-token-file <PATH>`
> (same JSON-RPC NDJSON protocol). This `code-control` entry remains a
> compatible legacy command until the W4-09 forwarding shim / W5-01 removal.

`libra code-control --stdio` is a local automation shim for an already running
`libra code --control write` session. It speaks newline-delimited JSON-RPC 2.0
on stdin/stdout and forwards requests to the loopback `/api/code/*` HTTP/SSE
control surface.

This command is not an MCP server. `libra code --stdio` remains the MCP stdio
transport and does not drive a live Code UI session. Do not confuse MCP
`--stdio` with `--control stdio`.

## Usage

```bash
# Canonical (preferred)
libra code --control stdio \
  --control-url http://127.0.0.1:3000 \
  --control-token-file .libra/code/control-token

# Legacy shim (still supported)
libra code-control --stdio \
  --url http://127.0.0.1:3000 \
  --token-file .libra/code/control-token
```

`--url` / `--control-url` should come from `.libra/code/control.json` and must
use a **literal loopback IP** (`http://127.0.0.1:…` or `http://[::1]:…`).
Hostnames such as `localhost` are rejected so DNS/hosts remapping cannot
redirect the token. `--token-file` / `--control-token-file` points at the
process-level token created by `libra code --control write`; the token is sent
as `X-Libra-Control-Token` for write-control HTTP requests. The HTTP client
disables proxies and redirects so the token cannot leave this machine.

## Methods

| JSON-RPC method | HTTP equivalent |
|-----------------|-----------------|
| `session.get` | `GET /api/code/session` |
| `events.subscribe` | `GET /api/code/events` as JSON-RPC notifications |
| `diagnostics.get` | `GET /api/code/diagnostics` |
| `controller.attach` | `POST /api/code/controller/attach` |
| `controller.detach` | `POST /api/code/controller/detach` |
| `message.submit` | `POST /api/code/messages` |
| `task.dispatch` | `POST /api/code/task/dispatch` |
| `interaction.respond` | `POST /api/code/interactions/{id}` |
| `turn.cancel` | `POST /api/code/control/cancel` |
| `goal.start` | `POST /api/code/goal/start` |
| `goal.status` | `GET /api/code/goal/status` |
| `goal.cancel` | `POST /api/code/goal/cancel` |

## Examples

Attach automation:

```json
{"jsonrpc":"2.0","id":1,"method":"controller.attach","params":{"clientId":"local-script","kind":"automation"}}
```

Submit a message after attach returns `controllerToken`:

```json
{"jsonrpc":"2.0","id":2,"method":"message.submit","params":{"controllerToken":"...","text":"/chat hello"}}
```

Dispatch a sub-agent explicitly:

```json
{"jsonrpc":"2.0","id":3,"method":"task.dispatch","params":{"controllerToken":"...","agent":"explorer","prompt":"grep TODO src/"}}
```

Respond to a pending interaction:

```json
{"jsonrpc":"2.0","id":4,"method":"interaction.respond","params":{"controllerToken":"...","interactionId":"interaction-1","response":{"approved":true}}}
```

Subscribe to events:

```json
{"jsonrpc":"2.0","id":5,"method":"events.subscribe"}
```

The shim first returns `{"subscribed":true}` and then emits notifications:

```json
{"jsonrpc":"2.0","method":"events.notification","params":{"event":"session_updated","data":{}}}
```

## Errors

Malformed JSON maps to JSON-RPC `-32700`. Unknown methods map to `-32601`.
Invalid params map to `-32602`. HTTP 4xx/5xx errors map to `-32000` with
`data.status` and `data.code`, preserving Libra errors such as
`INVALID_CONTROL_TOKEN`, `INVALID_CONTROLLER_TOKEN`, `CONTROLLER_CONFLICT`, and
`INTERACTION_NOT_ACTIVE`.

| Code | HTTP | Meaning |
|------|------|---------|
| `PLAN_REPAIR_RETRY_LIMIT_REACHED` | 409 | A plan-repair Continue request did not raise the exhausted automatic retry cap. Retry with a higher `maxAttempts` (for example, `{ "selectedOption": "continue", "maxAttempts": 3 }` when the current limit is 2), provide manual revision guidance, or cancel the repair. |
