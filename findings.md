# Findings & Decisions

## Requirements
- `/v1/messages` 请求遇到上游返回 `HTTP 400` 且错误体表示“模型负载过高/维护中”等临时问题时，应自动重试/切换到下一个可用上游，而不是直接失败。
- 期望存在“下一个上游”时一定会被尝试（同 provider 轮换或跨 provider 降级）。

## Research Findings
- 当前重试触发点在 `is_retryable_status()` 与 `is_retryable_error()`；`AttemptOutcome::Retryable` 才会继续尝试同优先级组内的其它 upstream。  
  关键限制：重试只发生在“同一个 provider 的 upstream 列表”中。
- `/v1/messages` 的 dispatch plan 会在请求开始时选择一个 provider（通常是 `anthropic` 或 `kiro`），之后 `forward_upstream_request()` 只会在该 provider 内轮换 upstream，不会跨 provider。
- 已实现 `/v1/messages` 的跨 provider fallback：当命中的 provider 被耗尽且结果仍是“可重试”（retryable）时，会自动尝试另一个 native provider（Anthropic ↔ Kiro）。
- 用户的运行时配置（`~/Library/Application Support/com.mxyhi.token-proxy/config.jsonc`）里：
  - `anthropic` provider 只有一个启用 upstream：`id=88code-claude`
  - `kiro` provider 有两个启用 upstream
  因此：即使把 400 判为可重试，`anthropic` 也没有“下一个 upstream”可切换。
- 日志（sqlite `request_logs`）里能看到该错误的 status=400、provider=anthropic、upstream_id=88code-claude，且 response_error 记录了错误 JSON（包含 request_id）。
- 目前安装包版本为 `0.1.29`（`/Applications/Token Proxy.app`），代码变更需通过 dev 启动/打包或后续 release 才会生效。

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| 引入 `/v1/messages` 跨 provider fallback | 同 provider 无备用 upstream 时，“可重试”也无法实际重试；用户期望有下一个上游就会被尝试 |
| 400 视为可重试（全量） | 与 new-api 策略对齐；用户场景中 400 代表“维护/过载”，需要触发轮换/降级 |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| “我明明有下一个上游，但还是不重试” | 根因是“下一个上游在另一个 provider（Kiro）”，当前实现不支持跨 provider fallback |

## Resources
- 配置路径：`~/Library/Application Support/com.mxyhi.token-proxy/config.jsonc`
- 数据库日志：`~/Library/Application Support/com.mxyhi.token-proxy/data.db` 表 `request_logs`

## Visual/Browser Findings
- 无
