# Token Proxy

[English](README.md) | 中文

中转 ai api 的工具，比如转发 openai api 格式，gemini ai api 格式，Anthropic api 格式，在本地运行，用于统计总 token 用量的、也可以负载均衡、优先级之类的

## macOS 安装

1. 下载应用并拖到 `/Applications`。
2. 若系统提示无法打开，执行：

```bash
xattr -cr /Applications/Token\ Proxy.app
```

## 配置说明

- 配置文件：`config.jsonc`（支持注释与尾随逗号）。
- 位置：Tauri 配置目录。
- 保存会重写文件；仅保留文件头部注释块。

示例：

```jsonc
{
  "host": "127.0.0.1",
  "port": 9208,
  "local_api_key": null,
  "log_path": "proxy.log",
  "log_level": "silent",
  "enable_api_format_conversion": false,
  "upstream_strategy": "priority_round_robin",
  "upstreams": [
    {
      "id": "openai-default",
      "provider": "openai",
      "base_url": "https://api.openai.com",
      "api_key": null,
      "priority": 0,
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
      "enabled": true
    },
    {
      "id": "anthropic-default",
      "provider": "anthropic",
      "base_url": "https://api.anthropic.com",
      "api_key": null,
      "priority": 0,
      "enabled": true
    },
    {
      "id": "gemini-default",
      "provider": "gemini",
      "base_url": "https://generativelanguage.googleapis.com",
      "api_key": null,
      "priority": 0,
      "enabled": true
    }
  ]
}
```

说明：
- 路由规则内置：`/v1/chat/completions` → `openai`，`/v1/responses` → `openai-response`，`/v1/messages`（及子路径）/`/v1/complete` → `anthropic`，`/v1beta/models/*:generateContent`/`*:streamGenerateContent` → `gemini`；OpenAI Chat/Responses 互转由 `enable_api_format_conversion` 控制（默认：`false`）。Anthropic/Gemini 不做格式转换。
- Anthropic 鉴权使用 `x-api-key`；当请求未携带 `anthropic-version` 时，代理默认补 `2023-06-01`（可被请求头覆盖）。
- Gemini（Google 官方 Gemini API）鉴权使用 query 参数 `key`（若请求未携带且 upstream 配置了 `api_key`，代理会自动补齐）；流式为 SSE，支持从 `usageMetadata` 统计 token。
- `log_level` 控制运行时 tracing 日志输出（默认：`silent`，即不输出）。可选值：`silent`/`error`/`warn`/`info`/`debug`/`trace`。
  - 当 `log_level` 为 `debug`/`trace` 时，代理会输出请求 header（鉴权相关会打码）和小体积请求体（最多 64KiB）。
- `priority` 越大优先级越高；同优先级时按配置文件中的列表顺序。
- `enabled` 用于禁用某个 upstream 而不删除；禁用的 upstream 不参与负载均衡。
- `model_mappings` 用于按 upstream 重写模型名（精确匹配、前缀通配 `*`、全量通配 `*`）；优先级：精确 > 前缀 > 通配；当映射生效时，响应会回写原始模型别名。

## Claude Code / Codex 接入

应用内「配置文件」页面提供一键写入自动配置：

- Claude Code：写入 `~/.claude/settings.json` 的 `env`（`ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`）
- Codex：写入 `~/.codex/config.toml` 的 `[model_providers.openai]`（`base_url`），并写入 `~/.codex/auth.json`（`OPENAI_API_KEY`）

写入前会生成 `.bak` 备份文件；修改后重启对应 CLI 生效。
