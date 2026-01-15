# Token Proxy

English | [中文](README.zh-CN.md)

Local AI API gateway for OpenAI / Gemini / Anthropic. Runs on your machine, keeps tokens counted (SQLite), offers priority-based load balancing, optional OpenAI Chat↔Responses format conversion, and one-click setup for Claude Code / Codex.

> Default listen port: **9208** (release) / **19208** (debug builds).

---

## What you get
- Multiple providers: `openai`, `openai-response`, `anthropic`, `gemini`
- Built-in routing + optional OpenAI Chat ⇄ Responses conversion
- Per-upstream priority + two balancing strategies (fill-first / round-robin)
- Model alias mapping (exact / prefix* / wildcard*) and response model rewrite
- Local access key (Authorization) + upstream key injection
- SQLite-powered dashboard (requests, tokens, cached tokens, latency, recent)
- macOS tray live token rate (optional)

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

## Configuration reference
- File: `config.jsonc` (comments + trailing commas allowed)
- Location: Tauri **AppConfig** directory (resolved automatically by the app)

### Core fields
| Field | Default | Notes |
| --- | --- | --- |
| `host` | `127.0.0.1` | Listen address (IPv6 allowed; will be bracketed in URLs) |
| `port` | `9208` release / `19208` debug | Change if the port is taken |
| `local_api_key` | `null` | When set: incoming requests must send `Authorization: Bearer <key>`; the same header will **not** forward upstream. |
| `app_proxy_url` | `null` | Proxy for app updater & as placeholder for upstreams (`"$app_proxy_url"`). Supports `http/https/socks5/socks5h`. |
| `log_level` | `silent` | `silent|error|warn|info|debug|trace`; debug/trace log request headers (auth redacted) and small bodies (≤64KiB). Release builds force `silent`. |
| `max_request_body_bytes` | `20971520` (20 MiB) | 0 = fallback to default. Protects inbound body size. |
| `tray_token_rate.enabled` | `true` | macOS tray live rate; harmless elsewhere. |
| `tray_token_rate.format` | `split` | `combined` (`total`), `split` (`↑in ↓out`), `both` (`total | ↑in ↓out`). |
| `enable_api_format_conversion` | `false` | Allow OpenAI Chat↔Responses and Anthropic Messages↔OpenAI Responses fallback with body/stream conversion. |
| `upstream_strategy` | `priority_fill_first` | `priority_fill_first` (default) keeps trying the highest-priority group in list order; `priority_round_robin` rotates within each priority group. |

### Upstream entries (`upstreams[]`)
| Field | Default | Notes |
| --- | --- | --- |
| `id` | required | Unique per upstream. |
| `provider` | required | One of `openai`, `openai-response`, `anthropic`, `gemini`. |
| `base_url` | required | Full base; overlapping path parts are de-duplicated. |
| `api_key` | `null` | Provider-specific bearer/key; overrides request headers. |
| `proxy_url` | `null` | Per-upstream proxy; supports `http/https/socks5/socks5h`; default is **no system proxy**. `$app_proxy_url` placeholder allowed. |
| `priority` | `0` | Higher = tried earlier. Grouped by priority then by order (or round-robin). |
| `enabled` | `true` | Disabled upstreams are skipped. |
| `model_mappings` | `{}` | Exact / `prefix*` / `*`. Priority: exact > longest prefix > wildcard. Response echoes original alias. |
| `overrides.header` | `{}` | Set/remove headers (null removes). Hop-by-hop/Host/Content-Length are always ignored. |

## Routing & format conversion
- Gemini: `/v1beta/models/*:generateContent` and `*:streamGenerateContent` → `gemini` (SSE supported).
- Anthropic: `/v1/messages` (and subpaths) and `/v1/complete` → `anthropic`.
- OpenAI: `/v1/chat/completions` → `openai`; `/v1/responses` → `openai-response`.
- Other paths: first provider with upstreams wins (prefers `openai`, then `openai-response`, then `anthropic`).
- If the preferred provider is missing but `enable_api_format_conversion=true`, the proxy auto-converts request/response bodies and streams between supported formats.
- If `anthropic` is missing for `/v1/messages` but `openai-response` exists and `enable_api_format_conversion=true`, the proxy auto-converts between Claude Messages and OpenAI Responses (including SSE).
- If `openai-response` is missing for `/v1/responses` but `anthropic` exists and `enable_api_format_conversion=true`, the proxy auto-converts between OpenAI Responses and Claude Messages (including SSE).

## Auth rules (important)
- Local access: `local_api_key` enabled → require `Authorization: Bearer <key>`; this header will **not** be forwarded upstream.
- Upstream auth resolution (per request):
  - **OpenAI**: `upstream.api_key` → `x-openai-api-key` → `Authorization` (only if `local_api_key` is **not** set) → error.
  - **Anthropic**: `upstream.api_key` → `x-api-key` / `x-anthropic-api-key` → error. Missing `anthropic-version` is auto-filled with `2023-06-01`.
  - **Gemini**: `upstream.api_key` → `x-goog-api-key` → query `?key=...` → error.

## Load balancing & retries
- Priorities: higher `priority` groups first; inside a group use list order (fill-first) or round-robin (if `priority_round_robin`).
- Retryable conditions: network timeout/connect errors, or status 403/429/307/5xx **except** 504/524. Retries stay within the same priority group; then the next lower priority group is tried.

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
- **Got 401?** If `local_api_key` is set, you must send `Authorization: Bearer <key>`; upstream keys go to `x-openai-api-key` / `x-api-key` / `x-goog-api-key` / `?key=`.
- **413 Payload Too Large?** Body exceeded `max_request_body_bytes` (default 20 MiB) or the 4 MiB transform limit for format-conversion requests.
- **Why no system proxy?** By design, `reqwest` is built with `.no_proxy()`; set per-upstream `proxy_url` if needed.
