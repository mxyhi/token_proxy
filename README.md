# Token Proxy

English | [中文](README.zh-CN.md)

Local AI API gateway for OpenAI / Gemini / Anthropic. Runs on your machine, keeps tokens counted (SQLite), offers priority-based load balancing, optional API format conversion (OpenAI Chat/Responses ↔ Anthropic Messages, plus Gemini ↔ OpenAI/Anthropic, including SSE/tools/images), and one-click setup for Claude Code / Codex.

> Default listen port: **9208** (release) / **19208** (debug builds).

---

## What you get
- Multiple providers: `openai`, `openai-response`, `anthropic`, `gemini`, `kiro`, `codex`, `antigravity`
- Built-in routing + optional format conversion (OpenAI Chat ⇄ Responses; Anthropic Messages ↔ OpenAI; Gemini ↔ OpenAI/Anthropic; SSE supported)
- Per-upstream priority + two balancing strategies (fill-first / round-robin)
- Model alias mapping (exact / prefix* / wildcard*) and response model rewrite
- Local access key (Authorization) + upstream key injection
- SQLite-powered dashboard (requests, tokens, cached tokens, latency, recent)
- macOS tray live token rate (optional)

## Screenshots
|  |  |
| --- | --- |
| **Dashboard**<br>![Dashboard](images/dashboard.png) | **Core**<br>![Core settings](images/core.png) |
| **Upstreams**<br>![Upstreams](images/upstream.png) | **Add upstream**<br>![Add upstream](images/add-upstream.png) |

## Quick start (macOS)
1) Install: move `Token Proxy.app` to `/Applications`. If blocked: `xattr -cr /Applications/Token\ Proxy.app`.
2) Launch the app. The proxy starts automatically.
3) Open **Config File** tab, edit and save (writes `config.jsonc` in the Tauri config dir). Defaults are usable; just paste your upstream API keys.
4) Call via curl (example with local auth):
```bash
curl -X POST \
  -H "Authorization: Bearer YOUR_LOCAL_KEY" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:9208/v1/chat/completions \
  -d '{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"hi"}]}'
```

You can also call using the Anthropic Messages format (useful for Claude Code clients):
```bash
curl -X POST \
  -H "x-api-key: YOUR_LOCAL_KEY" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:9208/v1/messages \
  -d '{"model":"claude-3-5-sonnet-20241022","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}'
```

## Workspace & CLI (Rust)
- This repo is now a Cargo workspace; the Tauri app still lives in `src-tauri/`.
- CLI crate: `crates/token_proxy_cli` (binary `token-proxy`).
- Default config path: `./config.jsonc` (override with `--config`).

```bash
# start proxy
cargo run -p token_proxy_cli -- serve

# start with custom config path
cargo run -p token_proxy_cli -- --config ./config.jsonc serve

# config helpers
cargo run -p token_proxy_cli -- config init
cargo run -p token_proxy_cli -- --config ./config.jsonc config path
```

## Frontend tests
```bash
# watch mode
pnpm test

# run once (CI-friendly)
pnpm test:run

# coverage (optional)
pnpm test:coverage

# TypeScript typecheck
pnpm exec tsc --noEmit
```

Notes:
- Test files live in `src/**/*.test.{ts,tsx}`.
- Global test setup (Tauri mocks + jsdom polyfills) is in `src/test/setup.ts`.
- Vitest config is in `vitest.config.ts`.

## Configuration reference
- File: `config.jsonc` (comments + trailing commas allowed)
- Location:
  - CLI: `--config` (default: `./config.jsonc`)
  - Tauri: **AppConfig** directory (resolved automatically by the app)

### Core fields
| Field | Default | Notes |
| --- | --- | --- |
| `host` | `127.0.0.1` | Listen address (IPv6 allowed; will be bracketed in URLs) |
| `port` | `9208` release / `19208` debug | Change if the port is taken |
| `local_api_key` | `null` | When set: local auth uses format-specific headers (see Auth rules); local auth inputs are **not** forwarded upstream. |
| `app_proxy_url` | `null` | Proxy for app updater & as placeholder for upstreams (`"$app_proxy_url"`). Supports `http/https/socks5/socks5h`. |
| `log_level` | `silent` | `silent|error|warn|info|debug|trace`; debug/trace log request headers (auth redacted) and small bodies (≤64KiB). Release builds force `silent`. |
| `max_request_body_bytes` | `20971520` (20 MiB) | 0 = fallback to default. Protects inbound body size. |
| `tray_token_rate.enabled` | `true` | macOS tray live rate; harmless elsewhere. |
| `tray_token_rate.format` | `split` | `combined` (`total`), `split` (`↑in ↓out`), `both` (`total | ↑in ↓out`). |
| `upstream_strategy` | `priority_fill_first` | `priority_fill_first` (default) keeps trying the highest-priority group in list order; `priority_round_robin` rotates within each priority group. |

### Upstream entries (`upstreams[]`)
| Field | Default | Notes |
| --- | --- | --- |
| `id` | required | Unique per upstream. |
| `providers` | required | One upstream can serve multiple providers. Special providers `kiro/codex/antigravity` cannot be mixed with others. |
| `base_url` | required | Full base; overlapping path parts are de-duplicated. (`providers=["kiro"]` / `["codex"]` / `["antigravity"]` can be empty.) |
| `api_key` | `null` | Provider-specific bearer/key; overrides request headers. |
| `kiro_account_id` | `null` | Required when `providers=["kiro"]`. |
| `preferred_endpoint` | `null` | `kiro` only (`providers=["kiro"]`): `ide` or `cli`. |
| `proxy_url` | `null` | Per-upstream proxy; supports `http/https/socks5/socks5h`; default is **no system proxy**. `$app_proxy_url` placeholder allowed. |
| `priority` | `0` | Higher = tried earlier. Grouped by priority then by order (or round-robin). |
| `enabled` | `true` | Disabled upstreams are skipped. |
| `model_mappings` | `{}` | Exact / `prefix*` / `*`. Priority: exact > longest prefix > wildcard. Response echoes original alias. |
| `convert_from_map` | `{}` | Explicitly allow inbound format conversion per provider. Example: `{ "openai-response": ["openai_chat", "anthropic_messages"] }`. |
| `overrides.header` | `{}` | Set/remove headers (null removes). Hop-by-hop/Host/Content-Length are always ignored. |

## Routing & format conversion
- Gemini: `/v1beta/models/*:generateContent` and `*:streamGenerateContent` → `gemini` (SSE supported).
- Anthropic: `/v1/messages` (and subpaths) and `/v1/complete` → `anthropic` (Kiro shares the same format).
- OpenAI: `/v1/chat/completions` → `openai`; `/v1/responses` → `openai-response`.
- Other paths: choose the provider with the highest configured priority; tie-break is `openai` > `openai-response` > `anthropic`.
- Cross-format fallback/conversion is controlled by `upstreams[].convert_from_map` (no global switch). If a provider has no eligible upstream for the inbound format, it won't be selected.
- If `openai` is missing for `/v1/chat/completions`: fallback can be `openai-response`, `anthropic`, or `gemini` (priority-based; tie-break prefers `openai-response`).
- For `/v1/messages`: choose between `anthropic` and `kiro` by priority; tie-break uses upstream id. If the chosen provider returns a retryable error, the proxy will fall back to the other native provider (Anthropic ↔ Kiro) when configured.
- If neither `anthropic` nor `kiro` exists for `/v1/messages`:
  - `antigravity` is supported by default (no `convert_from_map` needed; aligned with CLIProxyAPIPlus Antigravity/Claude Code behavior).
  - Other providers can be selected only when allowed for `anthropic_messages` via `convert_from_map` (e.g. `openai-response`, `openai`, `gemini`).
- If `openai-response` is missing for `/v1/responses`: fallback can be `openai`, `anthropic`, or `gemini` (priority-based; tie-break prefers `openai`).
- If `gemini` is missing for `/v1beta/models/*:generateContent`: fallback can be `openai-response`, `openai`, or `anthropic` (priority-based; tie-break prefers `openai-response`).

## Auth rules (important)
- Local access: `local_api_key` enabled → require format-specific key. These local auth inputs are stripped and **not** forwarded upstream.
  - OpenAI / Responses: `Authorization: Bearer <key>`
  - Anthropic `/v1/messages`: `x-api-key` or `x-anthropic-api-key`
  - Gemini: `x-goog-api-key` or `?key=...`
- When `local_api_key` is enabled, request headers are **not** used for upstream auth; configure `upstreams[].api_key` instead.
- Upstream auth resolution (per request):
  - **OpenAI**: `upstream.api_key` → `x-openai-api-key` → `Authorization` (only if `local_api_key` is **not** set) → error.
  - **Anthropic**: `upstream.api_key` → `x-api-key` / `x-anthropic-api-key` → error. Missing `anthropic-version` is auto-filled with `2023-06-01`.
  - **Gemini**: `upstream.api_key` → `x-goog-api-key` → query `?key=...` → error.

## Load balancing & retries
- Priorities: higher `priority` groups first; inside a group use list order (fill-first) or round-robin (if `priority_round_robin`).
- Retryable conditions: network timeout/connect errors, or status 400/403/429/307/5xx **except** 504/524. Retries stay within the same provider's priority groups.
- `/v1/messages` only: after the chosen native provider is exhausted (retryable errors), the proxy can fall back to the other native provider (`anthropic` ↔ `kiro`) if it is configured.

## Observability
- SQLite log: `data.db` in config dir. Stores per-request stats (tokens, cached tokens, latency, model, upstream).
- Token rate: macOS tray shows live total or split rates (configurable via `tray_token_rate`).
- Debug/trace log bodies capped at 64KiB.

## Dashboard
- In-app **Dashboard** page visualizes totals, per-provider stats, time series, and recent requests (page size 50, offset supported).

## One-click CLI setup
- Claude Code: writes `~/.claude/settings.json` `env` (`ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN` when local key is set).
- Codex: writes `~/.codex/config.toml` `model_provider="token_proxy"` and `[model_providers.token_proxy].base_url` → `http://127.0.0.1:<port>/v1`; writes `~/.codex/auth.json` `OPENAI_API_KEY`.
- A `.token_proxy.bak` file is created before overwriting; restart the CLI to apply.

## FAQ
- **Port already in use?** Change `port` in `config.jsonc`; remember to update your client base URL.
- **Got 401?** If `local_api_key` is set, you must send the format-specific local key (OpenAI/Responses: `Authorization`, Anthropic: `x-api-key`, Gemini: `x-goog-api-key` or `?key=`). With local auth enabled, configure upstream keys in `upstreams[].api_key`.
- **Got 504?** Upstream did not send response headers or the first body chunk within 120s. For streaming responses, a 120s idle timeout between chunks may also close the connection.
- **413 Payload Too Large?** Body exceeded `max_request_body_bytes` (default 20 MiB) or the 4 MiB transform limit for format-conversion requests.
- **Why no system proxy?** By design, `reqwest` is built with `.no_proxy()`; set per-upstream `proxy_url` if needed.
