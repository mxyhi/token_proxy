# Token Proxy

[English](README.md) | 中文

本地 AI API 网关，支持 OpenAI / Gemini / Anthropic。本地运行、记录 Token（SQLite），按优先级负载均衡，支持可选的 API 格式互转（OpenAI Chat/Responses ↔ Anthropic Messages，Gemini ↔ OpenAI/Anthropic，含 SSE/工具/图片），并提供 Claude Code / Codex 一键配置。

> 默认监听端口：**9208**（release）/ **19208**（debug 构建）。

---

## 你能得到什么
- 多提供商：`openai`、`openai-response`、`anthropic`、`gemini`、`kiro`、`codex`、`antigravity`
- 内置路由，支持可选的 API 格式互转（OpenAI Chat ⇄ Responses；Anthropic Messages ↔ OpenAI；Gemini ↔ OpenAI/Anthropic，含 SSE）
- 上游优先级 + 两种策略（填满优先级组 / 轮询）
- 模型别名映射（精确 / 前缀* / 通配*），响应会回写原始别名
- 本地访问密钥（Authorization）+ 上游密钥自动注入
- SQLite 仪表盘：请求数、Token、缓存 Token、延迟、最近请求
- macOS 托盘实时 Token 速率（可选）

## 应用截图
|  |  |
| --- | --- |
| **仪表盘**<br>![仪表盘](images/dashboard.png) | **核心配置**<br>![核心配置](images/core.png) |
| **上游管理**<br>![上游管理](images/upstream.png) | **新增上游**<br>![新增上游](images/add-upstream.png) |

## 快速上手（macOS）
1) 安装：把 `Token Proxy.app` 放到 `/Applications`。若被拦截，执行 `xattr -cr /Applications/Token\ Proxy.app`。
2) 启动应用，代理会自动运行。
3) 打开 **Config File** 标签，编辑并保存（写入 Tauri 配置目录下的 `config.jsonc`）。默认配置可用，只需填入上游 API Key。若代理正在运行，保存后会按需自动 reload 或重启。
4) 发请求（本地鉴权示例）：
```bash
curl -X POST \
  -H "Authorization: Bearer 你的本地密钥" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:9208/v1/chat/completions \
  -d '{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"hi"}]}'
```

也可以直接用 Anthropic Messages 格式（用于 Claude Code 等客户端）：
```bash
curl -X POST \
  -H "x-api-key: 你的本地密钥" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:9208/v1/messages \
  -d '{"model":"claude-3-5-sonnet-20241022","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}'
```

## Workspace & CLI（Rust）
- 现在是 Cargo workspace；Tauri 仍在 `src-tauri/`。
- CLI crate：`crates/token_proxy_cli`（二进制名 `token-proxy`）。
- 默认配置路径：`./config.jsonc`（用 `--config` 覆盖）。
- GitHub Releases 也会按 target 发布 CLI 压缩包：
  - Unix：`token-proxy_cli_<version>_<target>.tar.gz`
  - Windows：`token-proxy_cli_<version>_<target>.zip`

```bash
# 启动代理
cargo run -p token_proxy_cli -- serve

# 使用自定义配置路径
cargo run -p token_proxy_cli -- --config ./config.jsonc serve

# 配置辅助命令
cargo run -p token_proxy_cli -- config init
cargo run -p token_proxy_cli -- --config ./config.jsonc config path
```

## 前端测试
```bash
# watch 模式
pnpm test

# 单次运行（CI 友好）
pnpm test:run

# 覆盖率（可选）
pnpm test:coverage

# TypeScript 类型检查
pnpm exec tsc --noEmit
```

说明：
- 测试文件约定：`src/**/*.test.{ts,tsx}`。
- 全局测试初始化（Tauri mocks + jsdom polyfills）：`src/test/setup.ts`。
- Vitest 配置：`vitest.config.ts`。

## 配置参考
- 文件：`config.jsonc`（支持注释与尾随逗号）
- 位置：
  - CLI：`--config`（默认：`./config.jsonc`）
  - Tauri：**AppConfig** 目录（应用自动解析）

### 核心字段
| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `host` | `127.0.0.1` | 监听地址；支持 IPv6（URL 会自动加方括号） |
| `port` | `9208`（release）/`19208`（debug） | 端口冲突时修改 |
| `local_api_key` | `null` | 设置后，本地鉴权按接口格式生效（见“鉴权规则”）；本地鉴权不会转给上游 |
| `app_proxy_url` | `null` | 应用更新 & 上游可复用的代理；支持 `http/https/socks5/socks5h`；可在 upstream `proxy_url` 用 `"$app_proxy_url"` 占位 |
| `log_level` | `silent` | `silent|error|warn|info|debug|trace`；debug/trace 会记录请求头（鉴权打码）与小体积请求体（≤64KiB）；release 强制 `silent` |
| `max_request_body_bytes` | `20971520` (20 MiB) | 0 表示回落到默认；保护入站体积 |
| `retryable_failure_cooldown_secs` | `15` | 对适合短时降级的可重试失败施加冷却窗口；`0` 表示关闭冷却。重载或重启运行中的代理会重置当前冷却状态 |
| `tray_token_rate.enabled` | `true` | macOS 托盘实时速率；其他平台无害 |
| `tray_token_rate.format` | `split` | `combined`(总数) / `split`(↑入 ↓出) / `both`(总数 | ↑入 ↓出) |
| `upstream_strategy` | `priority_fill_first` | `priority_fill_first` 默认先填满高优先级；`priority_round_robin` 在同组内轮询 |

### 上游条目（`upstreams[]`）
| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `id` | 必填 | 唯一 |
| `providers` | 必填 | 一个上游可同时服务多个 provider。特殊 provider（`kiro/codex/antigravity`）不可与其它 provider 混用。 |
| `base_url` | 必填 | 完整基址，重复路径段会去重（`providers=["kiro"]` / `["codex"]` / `["antigravity"]` 可为空） |
| `api_key` | `null` | 该 provider 的密钥；优先于请求头 |
| `kiro_account_id` | `null` | `providers=["kiro"]` 时必填 |
| `preferred_endpoint` | `null` | `kiro` 专用：`ide` 或 `cli` |
| `proxy_url` | `null` | 每个上游独立代理，支持 `http/https/socks5/socks5h`；默认**不走系统代理**；支持 `$app_proxy_url` |
| `priority` | `0` | 越大越先尝试；同组按列表顺序或轮询 |
| `enabled` | `true` | 可临时禁用上游 |
| `model_mappings` | `{}` | 精确 / `前缀*` / `*`；优先级：精确 > 最长前缀 > 通配；响应回写原始模型别名 |
| `convert_from_map` | `{}` | 显式声明允许从哪些入站格式转换后使用该 provider。例：`{ "openai-response": ["openai_chat", "anthropic_messages"] }` |
| `overrides.header` | `{}` | 设置/删除 header（null 表示删除）；hop-by-hop/Host/Content-Length 永远忽略 |

## 路由与格式转换
- Gemini：`/v1beta/models/*:generateContent`、`*:streamGenerateContent` → `gemini`（支持 SSE）
- Anthropic：`/v1/messages`（含子路径）与 `/v1/complete` → `anthropic`（Kiro 同格式）
- OpenAI：`/v1/chat/completions` → `openai`；`/v1/responses` → `openai-response`
- 其他路径：按已配置 provider 的最高优先级选择；优先级相同则按 `openai` > `openai-response` > `anthropic` 打破平局
- 跨格式 fallback/转换由 `upstreams[].convert_from_map` 控制（不再有全局开关）；若某个 provider 在该入站格式下没有任何可用 upstream，则不会被选中。
- `/v1/chat/completions` 缺少 `openai`：可 fallback 到 `openai-response` / `anthropic` / `gemini`（按优先级选择，平级优先 `openai-response`）
- `/v1/messages`：在 `anthropic` 与 `kiro` 间按优先级选择；平级按 upstream id 排序。若命中 provider 返回“可重试错误”，且另一个 native provider 已配置，则会自动 fallback（Anthropic ↔ Kiro）
- 当 `/v1/messages` 缺少 `anthropic` 且 `kiro` 也不存在时：
  - `antigravity`：默认支持（无需 `convert_from_map`，对齐 CLIProxyAPIPlus 的 Antigravity/Claude Code 体验）
  - 其它 provider：若在 `convert_from_map` 中允许 `anthropic_messages`，则可 fallback 到 `openai-response` / `openai` / `gemini`（按优先级选择，平级优先 `openai-response`）
- `/v1/responses` 缺少 `openai-response`：可 fallback 到 `openai` / `anthropic` / `gemini`（按优先级选择，平级优先 `openai`）
- `/v1beta/models/*:generateContent` 缺少 `gemini`：可 fallback 到 `openai-response` / `openai` / `anthropic`（按优先级选择，平级优先 `openai-response`）

## 鉴权规则（重要）
- 本地访问：设置了 `local_api_key` 必须按接口格式携带本地 key，且这些本地鉴权不会转发给上游
  - OpenAI / Responses：`Authorization: Bearer <key>`
  - Anthropic `/v1/messages`：`x-api-key` / `x-anthropic-api-key`
  - Gemini：`x-goog-api-key` 或 `?key=...`
- 启用 `local_api_key` 时，请求头不会用于上游鉴权；请在 `upstreams[].api_key` 配置上游 key
- 上游鉴权解析（逐请求）：
  - **OpenAI**：`upstream.api_key` → `x-openai-api-key` → `Authorization`（仅当未设置 `local_api_key`）→ 报错
  - **Anthropic**：`upstream.api_key` → `x-api-key` / `x-anthropic-api-key` → 报错；若缺少 `anthropic-version` 自动补 `2023-06-01`
  - **Gemini**：`upstream.api_key` → `x-goog-api-key` → 查询参数 `?key=` → 报错

## 负载均衡与重试
- 优先级：高优先级组先尝试；组内按列表顺序（fill-first）或轮询（round-robin）
- 可重试条件：网络超时/连接错误，或状态码 400/401/403/404/408/422/429/307/5xx（包含 504/524）；重试只在同一 provider 的优先级组内进行
- 冷却条件：`401/403/408/429/5xx` 会让失败 upstream 在 `retryable_failure_cooldown_secs`（默认 `15`）内被暂时后置；`400/404/422/307` 仍可重试，但不会触发跨请求冷却
- 仅 `/v1/messages`：当命中的 native provider（`anthropic`/`kiro`）被耗尽（仍是可重试错误）时，若另一个 native provider 已配置，会自动 fallback（Anthropic ↔ Kiro）

## 可观测性
- SQLite 日志：`data.db` 位于配置目录，记录每次请求（tokens、cached tokens、延迟、模型、上游）
- Token 速率：macOS 托盘可显示总速率或分向（由 `tray_token_rate` 决定）
- debug/trace 日志的请求体最大 64KiB

## Dashboard
- 应用内 **Dashboard** 展示总览、按 provider 统计、时间序列、最近请求（分页 50，支持 offset）
- Logs 面板支持“记录 30 秒内请求详情”：开启后会在 30 秒窗口内记录请求 header/body，失败请求的错误响应始终保留，到时自动关闭

## 一键写 CLI 配置
- Claude Code：写入 `~/.claude/settings.json` 的 `env`（`ANTHROPIC_BASE_URL`，若有本地密钥则写 `ANTHROPIC_AUTH_TOKEN`）
- Codex：写入 `~/.codex/config.toml` 的 `model_provider="token_proxy"` 与 `[model_providers.token_proxy].base_url` → `http://127.0.0.1:<port>/v1`；写入 `~/.codex/auth.json` 的 `OPENAI_API_KEY`
- OpenCode：写入 `~/.config/opencode/opencode.json[c]` 的 `provider.token_proxy` 与 `options.baseURL` → `http://127.0.0.1:<port>/v1`；写入 `~/.local/share/opencode/auth.json` 的 `token_proxy.key`。需要在 Upstreams 中至少配置一个**精确模型映射**（不能带 `*`）
- 写入前会生成 `.token_proxy.bak` 备份；写完重启对应 CLI 生效

## FAQ
- **端口被占用？** 修改 `config.jsonc` 里的 `port`，并同步更新客户端 base URL
- **返回 401？** 设置了 `local_api_key` 就必须按接口格式发送本地 key（OpenAI/Responses 用 `Authorization`；Anthropic 用 `x-api-key`；Gemini 用 `x-goog-api-key` 或 `?key=`）；开启本地鉴权后，上游密钥请配置在 `upstreams[].api_key`
- **返回 504？** 上游在 120 秒内未返回响应头或首个 body chunk。对于流式响应，若相邻 chunk 间空闲超过 120 秒，连接也可能被关闭。
- **413 Payload Too Large？** 请求体超过 `max_request_body_bytes`（默认 20 MiB）或格式转换场景的 4 MiB 处理上限
- **为什么不走系统代理？** `reqwest` 默认 `no_proxy()`；如需代理，请在每个 upstream 设置 `proxy_url`
