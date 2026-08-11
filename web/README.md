# Libra Code Web UI

This directory holds the Next.js source for the embedded `libra code` browser UI. The build is consumed two ways:

1. **`pnpm dev`** during local development serves the UI on the Next.js dev server's default `http://localhost:3000`. All API calls use relative `/api/...` paths with `same-origin` credentials (see `src/lib/code-ui/client.ts`), so the dev server must share its origin with a running `libra code` process. The typical workflow is to launch the backend on a non-default port — `libra code --web-only --port 4400` — and then run `pnpm dev -- --port 4400` so both share the loopback origin. There is **no** `LIBRA_DEV_API_BASE`-style env-var-based proxy: the client speaks to `/api/*` directly and the Rust side's `ensure_loopback_api_request` guard refuses remote callers regardless.
2. **`pnpm build`** emits a static export to `web/out/`. The Rust binary embeds that directory at compile time via `WebAssets` (`src/command/web_assets.rs`) and serves it from `axum::Router::fallback`. Any production change to the UI therefore requires `pnpm build` so the embedded snapshot stays current; CI fails closed if `web/out/` falls behind the source.

## Scripts

```bash
pnpm install        # install deps (uses pnpm-lock.yaml)
pnpm dev            # local dev server with HMR
pnpm lint           # eslint, no warnings allowed
pnpm test           # vitest unit tests for the browser foundation
pnpm build          # static export → web/out/
```

## Current UI status (W2-07 + W2-08)

The shipped page mounts the shared session/SSE store and browser-controller
lease provider, shows the current session title and phase, and renders the
pending approval / `request_user_input` panel when the snapshot has one
(`SessionInteractions` → `InteractionsHost`). It does not yet ship a
three-pane workspace, composer, thread sidebar, terminal, or workflow tabs.

W2-07 owns the shared wire foundation under `web/src/lib/code-ui/`. W2-08 owns
approval and structured user-input under `web/src/lib/code-ui/interactions/`
and `web/src/components/workspace/interactions/`. Later domain panels remain
with later W2 cards (usage W2-13, workflow review W2-16, etc.).

## Live API contract

The browser only talks to its same-origin server. The Rust side enforces loopback at every `/api/*` route, so this client does not host-check. Non-loopback HTML navigation receives the embedded `remote-notice/` static page instead of the SPA, and non-loopback asset/API fallbacks return 404. Source of truth: `src/internal/ai/web/mod.rs`.

| Endpoint | Verb | Purpose |
|----------|------|---------|
| `/api/health` | GET | Liveness probe — returns plain `"ok"`. Cheapest sanity check that the embedded server is bound. |
| `/api/repo` | GET | Repository identity (`id`, `name`, `description`). |
| `/api/repo/status` | GET | Working-tree status — same JSON envelope as `libra status --json` (`{ ok, command: "status", data }`). |
| `/api/code/session` | GET | Initial `CodeUiSessionSnapshot`. |
| `/api/code/events` | GET (SSE) | `session_updated` / `status_changed` / `controller_changed` frames; server lag emits a full `session_updated` snapshot, and clients fall back to `GET /api/code/session` on disconnect. |
| `/api/code/threads?limit&offset` | GET | Active thread projections for the sidebar (`{ items, nextOffset }`). |
| `/api/code/diagnostics` | GET | Redacted runtime info (PID, ports, log file, controller). |
| `/api/code/controller/attach` | POST | Issue a lease (`{ clientId, kind: "browser" }`). Returns `controllerToken`. |
| `/api/code/controller/detach` | POST | Release the lease (header `X-Code-Controller-Token`). |
| `/api/code/messages` | POST | Submit a user message (header `X-Code-Controller-Token`, body ≤256 KiB). |
| `/api/code/interactions/{id}` | POST | Resolve a pending `CodeUiInteractionRequest`. |
| `/api/code/control/cancel` | POST | Cancel the active turn. Browser leases need only the controller token; automation leases additionally require `X-Libra-Control-Token`. |

The wire types are pinned in two places — keep them in lock-step:

- TypeScript: `web/src/lib/code-ui/types.ts`.
- Rust: `src/internal/ai/web/code_ui.rs` (`#[serde(rename_all = "camelCase")]` on every struct, `#[serde(rename_all = "snake_case")]` on every enum). The serde golden tests in `tests/ai_code_ui_wire_test.rs` fail loudly when the JSON shape drifts.

## Module layout

```
web/src/
├── app/                       # Next.js app router entry (mounts SessionInteractions)
├── components/workspace/
│   └── interactions/          # Approval + request_user_input panels (W2-08)
└── lib/
    └── code-ui/               # Shared wire types, client, store, controller (W2-07)
        └── interactions/      # Approval/user-input helpers + fixtures (W2-08)
```

`web/src/lib/code-ui/store.tsx` owns the `CodeUiSessionSnapshot` and the SSE reconnect loop. `web/src/lib/code-ui/controller.tsx` owns the browser controller lease. `app/page.tsx` mounts both providers once and renders `SessionInteractions` so pending approval/user-input prompts resolve through the leased controller.

## Browser write surface

Composer and other domain writers will continue to land in later W2 cards.
Approval and `request_user_input` already flow through `useBrowserController()`
via `SessionInteractions`: `respond` posts to `/api/code/interactions/{id}` and
`cancel` posts to `/api/code/control/cancel`. On the first write the hook calls
`POST /api/code/controller/attach`, caches `controllerToken` + `leaseExpiresAt`
in memory, and replays the original request. The browser `clientId` is
persisted in `sessionStorage` so an unexpected reload can renew the same
lease; an explicit detach still clears write access for a clean hand-off.

Recovery semantics in `controller.tsx`:

- `MISSING_CONTROLLER_TOKEN` / `INVALID_CONTROLLER_TOKEN` — clear cache, retry once.
- `CONTROLLER_CONFLICT` — surface the current owner; do not loop on retry.
- `BROWSER_CONTROL_DISABLED` — show a hint pointing to the `--browser-control loopback` CLI flag.
- `PAYLOAD_TOO_LARGE` — surfaced inline; the client also caps body at 256 KiB before posting.

`beforeunload` issues a best-effort `fetch("/api/code/controller/detach", { keepalive: true })` so the next browser session can attach without bumping into a stale lease. `navigator.sendBeacon` cannot set custom headers and is therefore not used for the detach call.

## Capability gating

Every writable control is gated on `snapshot.capabilities.*` plus `snapshot.controller.canWrite`. The current capability set is set by the Rust runtime: `--web-only --provider codex` advertises the full set, `HeadlessCodeRuntime` advertises `messageInput` + `streamingText` + `toolCalls`, and the read-only placeholder advertises none.

## Dev tips

- `pnpm dev` does not embed assets into the Rust binary; you'll see "Loading…" placeholders for any feature that depends on a live `libra code` API. Run a TUI session in another terminal so the SSE channel has data to stream.
- `pnpm build` and `cargo build` are independent — when you modify both layers, run `pnpm build` first so `web/out/` is up to date before the Rust crate compiles.
- The static export needs `output: "export"`, `trailingSlash: true`, and `images.unoptimized` (configured in `next.config.ts`). Don't toggle these without updating `WebAssets` accordingly.
