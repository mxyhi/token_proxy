# Token Proxy

[English](README.md) | 中文

本地 AI API 网关，支持 OpenAI / Gemini / Anthropic。本地运行、记录 Token（SQLite），按优先级负载均衡，可选 OpenAI Chat↔Responses 互转，并提供 Claude Code / Codex 一键配置。

> 默认监听端口：**9208**（release）/ **19208**（debug 构建）。

---

## 你能得到什么
- 多提供商：`openai`、`openai-response`、`anthropic`、`gemini`
- 内置路由，支持可选的 OpenAI Chat ⇄ Responses 自动转换
- 上游优先级 + 两种策略（填满优先级组 / 轮询）
- 模型别名映射（精确 / 前缀* / 通配*），响应会回写原始别名
- 本地访问密钥（Authorization）+ 上游密钥自动注入
- SQLite 仪表盘：请求数、Token、缓存 Token、延迟、最近请求
- macOS 托盘实时 Token 速率（可选）

## 快速上手（macOS）
1) 安装：把 `Token Proxy.app` 放到 `/Applications`。若被拦截，执行 `xattr -cr /Applications/Token\ Proxy.app`。
2) 启动应用，代理会自动运行。
3) 打开 **Config File** 标签，编辑并保存（写入 Tauri 配置目录下的 `config.jsonc`）。默认配置可用，只需填入上游 API Key。
4) 发请求（本地鉴权示例）：
```bash
curl -X POST \
  -H "Authorization: Bearer 你的本地密钥" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:9208/v1/chat/completions \
  -d '{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"hi"}]}'
```

## 配置参考
- 文件：`config.jsonc`（支持注释与尾随逗号）
- 位置：Tauri **AppConfig** 目录（应用自动解析）

### 核心字段
| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `host` | `127.0.0.1` | 监听地址；支持 IPv6（URL 会自动加方括号） |
| `port` | `9208`（release）/`19208`（debug） | 端口冲突时修改 |
| `local_api_key` | `null` | 设置后，入站必须带 `Authorization: Bearer <key>`；此头**不会**再转给上游 |
| `app_proxy_url` | `null` | 应用更新 & 上游可复用的代理；支持 `http/https/socks5/socks5h`；可在 upstream `proxy_url` 用 `"$app_proxy_url"` 占位 |
| `log_path` | `proxy.log` | 相对配置目录；**release 构建不写此文件**（仅写 SQLite） |
| `log_level` | `silent` | `silent|error|warn|info|debug|trace`；debug/trace 会记录请求头（鉴权打码）与小体积请求体（≤64KiB） |
| `max_request_body_bytes` | `20971520` (20 MiB) | 0 表示回落到默认；保护入站体积 |
| `tray_token_rate.enabled` | `true` | macOS 托盘实时速率；其他平台无害 |
| `tray_token_rate.format` | `split` | `combined`(总数) / `split`(↑入 ↓出) / `both`(总数 | ↑入 ↓出) |
| `enable_api_format_conversion` | `false` | 允许 OpenAI Chat↔Responses 自动 fallback 与流式/体转换 |
| `upstream_strategy` | `priority_fill_first` | `priority_fill_first` 默认先填满高优先级；`priority_round_robin` 在同组内轮询 |

### 上游条目（`upstreams[]`）
| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `id` | 必填 | 唯一 |
| `provider` | 必填 | `openai` / `openai-response` / `anthropic` / `gemini` |
| `base_url` | 必填 | 完整基址，重复路径段会去重 |
| `api_key` | `null` | 该 provider 的密钥；优先于请求头 |
| `proxy_url` | `null` | 每个上游独立代理，支持 `http/https/socks5/socks5h`；默认**不走系统代理**；支持 `$app_proxy_url` |
| `priority` | `0` | 越大越先尝试；同组按列表顺序或轮询 |
| `enabled` | `true` | 可临时禁用上游 |
| `model_mappings` | `{}` | 精确 / `前缀*` / `*`；优先级：精确 > 最长前缀 > 通配；响应回写原始模型别名 |
| `overrides.header` | `{}` | 设置/删除 header（null 表示删除）；hop-by-hop/Host/Content-Length 永远忽略 |

## 路由与格式转换
- Gemini：`/v1beta/models/*:generateContent`、`*:streamGenerateContent` → `gemini`（支持 SSE）
- Anthropic：`/v1/messages`（含子路径）与 `/v1/complete` → `anthropic`
- OpenAI：`/v1/chat/completions` → `openai`；`/v1/responses` → `openai-response`
- 其他路径：按已配置的 provider 依次匹配（优先 `openai`，再 `openai-response`，再 `anthropic`）
- 若缺少对应 OpenAI provider 且 `enable_api_format_conversion=true`，将自动在 Chat/Responses 之间转换请求与响应（含流式）

## 鉴权规则（重要）
- 本地访问：设置了 `local_api_key` 必须带 `Authorization: Bearer <key>`；此头不会被转发给上游
- 上游鉴权解析（逐请求）：
  - **OpenAI**：`upstream.api_key` → `x-openai-api-key` → `Authorization`（仅当未设置 `local_api_key`）→ 报错
  - **Anthropic**：`upstream.api_key` → `x-api-key` / `x-anthropic-api-key` → 报错；若缺少 `anthropic-version` 自动补 `2023-06-01`
  - **Gemini**：`upstream.api_key` → `x-goog-api-key` → 查询参数 `?key=` → 报错

## 负载均衡与重试
- 优先级：高优先级组先尝试；组内按列表顺序（fill-first）或轮询（round-robin）
- 可重试条件：网络超时/连接错误，或状态码 403/429/307/5xx（排除 504/524）；先在当前优先级组内重试，再降级到下一优先级组

## 可观测性
- SQLite 日志：`data.db` 位于配置目录，记录每次请求（tokens、cached tokens、延迟、模型、上游）
- `proxy.log`：仅 debug 构建写入；release 不写文件
- Token 速率：macOS 托盘可显示总速率或分向（由 `tray_token_rate` 决定）
- debug/trace 日志的请求体最大 64KiB

## Dashboard
- 应用内 **Dashboard** 展示总览、按 provider 统计、时间序列、最近请求（分页 50，支持 offset）

## 一键写 CLI 配置
- Claude Code：写入 `~/.claude/settings.json` 的 `env`（`ANTHROPIC_BASE_URL`，若有本地密钥则写 `ANTHROPIC_AUTH_TOKEN`）
- Codex：写入 `~/.codex/config.toml` 的 `[model_providers.openai].base_url` → `http://127.0.0.1:<port>/v1`；写入 `~/.codex/auth.json` 的 `OPENAI_API_KEY`
- 写入前会生成 `.bak` 备份；写完重启对应 CLI 生效

## FAQ
- **端口被占用？** 修改 `config.jsonc` 里的 `port`，并同步更新客户端 base URL
- **返回 401？** 设置了 `local_api_key` 就必须带 `Authorization: Bearer <key>`；上游密钥放 `x-openai-api-key` / `x-api-key` / `x-goog-api-key` / `?key=`
- **413 Payload Too Large？** 请求体超过 `max_request_body_bytes`（默认 20 MiB）或格式转换场景的 4 MiB 处理上限
- **为什么不走系统代理？** `reqwest` 默认 `no_proxy()`；如需代理，请在每个 upstream 设置 `proxy_url`
