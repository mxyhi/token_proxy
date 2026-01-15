# Task Plan: Chat ↔ Claude Messages 互转（对齐 new-api，含流式/图片/工具）

## Goal
在 `token_proxy` 中实现 OpenAI `/v1/chat/completions` ↔ Anthropic `/v1/messages` 的双向互转与自动 fallback（含 SSE 流式、图片、tools/tool_choice/parallel_tool_calls），行为尽量对齐 `QuantumNous/new-api`。

## Phases
- [ ] Phase 1: 明确方案细节与落点（含优先级规则），整理测试用例
- [ ] Phase 2: 请求体转换：Chat ↔ Responses（补齐 tool/image/tool_result/system），Responses ↔ Anthropic（system 按 new-api 输出为数组 blocks）
- [ ] Phase 3: 流式转换管道：统一错误类型以支持 Chat↔Claude 的组合流式转换；避免重复日志/重复 token 统计
- [ ] Phase 4: 路由 fallback：/v1/chat/completions 与 /v1/messages 双向 fallback（按“优先级”选择）
- [ ] Phase 5: 测试与回归：新增单测覆盖（请求体 + SSE）；跑 cargo test / tsc
- [ ] Phase 6: 文档/配置说明更新（README.zh-CN.md / README.md），收尾

## Key Questions
1. “按优先级”在跨 provider fallback 时的精确定义：是否按各 provider 的最高 upstream.priority 选择，平级再按默认顺序？
2. Chat↔Claude 的 system 归一化：对外（Claude）按 new-api 输出 `system: [{type:'text',text:...}]`；对内（Responses）用 `instructions`，是否需要保留分段信息？
3. 图片与文件：Chat `image_url` ↔ Claude `image`，以及 `input_file/document` 是否一并对齐 new-api？

## Decisions Made
- 采用方案 A：以 OpenAI Responses 作为内部中间格式，复用既有转换链，避免双份映射逻辑。
- system 输出按 new-api：Claude 请求的 `system` 使用数组 blocks（`[{type:'text',text:...}]`），并支持输入为 string/array 两种形式。
- 跨 provider fallback “按优先级”定义：在候选 provider 中选取其 `ProviderUpstreams.groups[0].priority` 最大者；若相同则按固定顺序打破平局（/v1/chat：openai-response > anthropic；/v1/messages：openai-response > openai；/v1/responses：openai > anthropic）。

## Errors Encountered
- None yet

## Status
**Currently in Phase 1** - 梳理“优先级”规则与代码落点，准备写入实现与测试。
