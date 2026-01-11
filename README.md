# Token Proxy

English | [中文](README.zh-CN.md)

A tool for proxying AI APIs, such as forwarding OpenAI API format, Gemini AI API format, Anthropic API format, running locally, used for counting total token usage, and also for load balancing, priority management, and similar functions.

## macOS Installation

1. Download the app and move it to `/Applications`.
2. If macOS blocks the app, run:

```bash
xattr -cr /Applications/Token\ Proxy.app
```

## Configuration

- Config file: `config.jsonc` (JSONC with comments and trailing commas).
- Location: Tauri config directory.
- Saving rewrites the file; the leading comment block is preserved.

Example:

```jsonc
{
  "host": "127.0.0.1",
  "port": 9208,
  "local_api_key": null,
  "log_path": "proxy.log",
  "enable_api_format_conversion": false,
  "upstream_strategy": "priority_round_robin",
  "upstreams": [
    {
      "id": "openai-default",
      "provider": "openai",
      "base_url": "https://api.openai.com",
      "api_key": null,
      "priority": 0,
      "index": 0,
      "enabled": true,
      "model_mappings": {
        "gpt-4": "gpt-4.1",
        "gpt-4*": "gpt-4.1-mini",
        "*": "gpt-4.1-mini"
      }
    },
    {
      "id": "openai-responses",
      "provider": "openai-response",
      "base_url": "https://api.openai.com",
      "api_key": null,
      "priority": 0,
      "index": 1,
      "enabled": true
    },
    {
      "id": "anthropic-default",
      "provider": "anthropic",
      "base_url": "https://api.anthropic.com",
      "api_key": null,
      "priority": 0,
      "index": 2,
      "enabled": true
    },
    {
      "id": "gemini-default",
      "provider": "gemini",
      "base_url": "https://generativelanguage.googleapis.com",
      "api_key": null,
      "priority": 0,
      "index": 3,
      "enabled": true
    }
  ]
}
```

Notes:
- Request routing is built in: `/v1/chat/completions` → `openai`, `/v1/responses` → `openai-response`, `/v1/messages` (and subpaths) / `/v1/complete` → `anthropic`, `/v1beta/models/*:generateContent` / `*:streamGenerateContent` → `gemini`. OpenAI Chat/Responses conversion is controlled by `enable_api_format_conversion` (default: `false`). Anthropic/Gemini are pass-through (no format conversion).
- Anthropic auth uses `x-api-key`. If `anthropic-version` is missing, the proxy injects `2023-06-01` (override by providing the header explicitly).
- Gemini (Google AI Studio Gemini API) auth uses query parameter `key` (if missing and `api_key` is configured on the upstream, the proxy injects it). Streaming is SSE; token usage is extracted from `usageMetadata` when present.
- `priority` sorts descending; `index` sorts ascending inside the same priority group.
- Missing `index` values are auto-assigned globally after the current max index when saving.
- `enabled` disables an upstream without deleting it; disabled upstreams are ignored during load balancing.
- `model_mappings` rewrites model names per upstream (exact match, prefix with `*`, wildcard `*`). Priority: exact > prefix > wildcard. Responses return the original model alias when a mapping applies.
