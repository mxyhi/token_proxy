# Task Plan: /v1/messages 400 未触发重试/降级

## Goal
当 `/v1/messages` 的首选上游返回“可重试错误”（例如：HTTP 400 但错误体提示模型维护/负载过高）时，代理应自动尝试下一个可用上游（包括跨 provider 的 fallback），而不是直接把该错误返回给客户端。

## Current Phase
Phase 5

## Phases

### Phase 1: Requirements & Discovery
- [x] 收集用户报错样本与请求路径（`/v1/messages`）
- [x] 定位当前重试策略与触发条件
- [x] 确认配置中是否存在“下一个上游”
- [x] 记录关键发现到 findings.md
- **Status:** complete

### Phase 2: Planning & Structure
- [x] 明确“重试”的语义：优先同 provider 轮换；`/v1/messages` 额外支持跨 provider fallback
- [x] 设计 `/v1/messages` 的 provider fallback 顺序与边界（Anthropic ↔ Kiro）
- [x] 明确 400 的可重试判定：采用“全量 400 可重试”（与 new-api 对齐）
- [x] 设计可观测性：日志可看到每次尝试的 provider/upstream_id
- **Status:** complete

### Phase 3: Implementation
- [x] 实现 `/v1/messages` 的跨 provider fallback（Anthropic→Kiro 或反之）
- [x] 调整 retryable 判定（把 400 纳入 retryable status）
- [x] 保持请求体可重放（`forward_upstream_request` 改为引用参数，复用 ReplayableBody）
- **Status:** complete

### Phase 4: Testing & Verification
- [x] 运行 `cargo test -p token_proxy_core`
- [x] 运行 `cargo test -p token_proxy_cli`
- [x] 运行 `cd src-tauri && cargo test`
- **Status:** complete

### Phase 5: Delivery
- [x] 更新 README（重试与 fallback 的真实行为）
- [ ] 推送分支并更新 PR
- [ ] 指导用户如何验证（本地跑 dev / 打包 / release）
- **Status:** in_progress

## Key Questions
1. 对于 HTTP 400：是“全部可重试”，还是仅当错误体匹配“临时不可用/维护/过载”等特征才可重试？（已选：全部可重试）
2. `/v1/messages` 的 provider fallback 是否仅限 `anthropic ↔ kiro`，还是允许在开启格式转换时继续 fallback 到 `openai-response/openai/gemini`？（已选：仅限 native providers）
3. 当首选 provider 内存在多个 upstream 时，优先“同 provider 轮换”还是“跨 provider 降级”？（已选：先同 provider 轮换，再跨 provider fallback）
4. 是否需要为“已重试过的 upstream/provider”打标，避免循环尝试？（已选：最多一次跨 provider fallback，不做循环）

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| `/v1/messages` 需要跨 provider fallback | 现有重试只在同 provider 内轮换；配置里 Anthropic 仅 1 个 upstream，导致 400 也无法“重试下一个” |
| 400 视为可重试（全量） | 与 new-api 策略对齐；且用户场景中 400 用于“维护/过载”，需要触发轮换/降级 |

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| `/v1/messages` 返回 400 未触发重试 | 1 | 已定位：缺少跨 provider fallback；仅 Anthropic 组内无下一上游 |
