# Task Plan: Gemini ↔ Responses 双向转换与展示更新

## Goal
完成 Gemini ↔ OpenAI Responses 双向请求/响应/流式转换，更新文档与前端说明。

## Phases
- [x] Phase 1: Plan and setup
- [x] Phase 2: Research/gather information
- [x] Phase 3: Execute/build
- [x] Phase 4: Review and deliver

## Key Questions
1. Gemini ↔ Responses 的映射规则是否按 new-api 行为实现？
2. Gemini 路径在无 upstream 时是否允许回退到 Responses/Chat？
3. response_format 与 responseMimeType/responseSchema 如何互转？

## Decisions Made
- 采用双阶段链路（Responses ↔ Chat ↔ Gemini）复用现有转换逻辑。
- 开启 Gemini 路由 fallback（需 enable_api_format_conversion）。
- UI 与 README 仅做最小说明更新（方案 A）。

## Errors Encountered
- cargo test 编译错误（Option unwrap_or 类型不匹配、函数签名不兼容、Option<&str> 匹配分支错误）：已修复。

## Status
**Completed** - Gemini ↔ Responses 双向转换、路由回退、README 与前端说明已完成。

---

转换实现情况矩阵
┌───────────────────────┬─────────────┬───────────┬─────────────┬───────────┐
│ 源格式 ↓ / 目标格式 → │ Chat        │ Responses │ Anthropic   │ Gemini    │
├───────────────────────┼─────────────┼───────────┼─────────────┼───────────┤
│ Chat                  │ -           │ ✅ 已实现  │ ✅ 中转实现  │ ✅ 已实现  │
├───────────────────────┼─────────────┼───────────┼─────────────┼───────────┤
│ Responses             │ ✅ 已实现    │ -         │ ✅ 已实现    │ ✅ 已实现  │
├───────────────────────┼─────────────┼───────────┼─────────────┼───────────┤
│ Anthropic             │ ✅ 中转实现  │ ✅ 已实现  │ -           │ ❌ 未实现  │
├───────────────────────┼─────────────┼───────────┼─────────────┼───────────┤
│ Gemini                │ ✅ 已实现    │ ✅ 已实现  │ ❌ 未实现    │ -         │
└───────────────────────┴─────────────┴───────────┴─────────────┴───────────┘

新增实现 (本次)
┌──────┬────────────────────────────┬──────────────────────────────────────────────────────┐
│ 编号 │ 转换方向                   │ 说明                                                 │
├──────┼────────────────────────────┼──────────────────────────────────────────────────────┤
│ 1    │ Gemini ↔ Responses          │ ✅ 请求/响应/流式通过 Chat 中转                       │
├──────┼────────────────────────────┼──────────────────────────────────────────────────────┤
│ 2    │ Gemini 请求 → Chat          │ ✅ Gemini 路径 fallback 到 OpenAI/Responses           │
├──────┼────────────────────────────┼──────────────────────────────────────────────────────┤
│ 3    │ Chat 响应 → Gemini          │ ✅ Gemini fallback 输出格式化                          │
├──────┼────────────────────────────┼──────────────────────────────────────────────────────┤
│ 4    │ Chat 流式 → Gemini          │ ✅ OpenAI SSE → Gemini SSE                             │
└──────┴────────────────────────────┴──────────────────────────────────────────────────────┘

未实现的转换 (共 2 种)
┌──────┬────────────────────┬────────────────────────────────────────────────┐
│ 编号 │ 转换方向           │ 说明                                           │
├──────┼────────────────────┼────────────────────────────────────────────────┤
│ 1    │ Gemini → Anthropic │ 无法将 Gemini 响应转为 Anthropic Messages 格式 │
├──────┼────────────────────┼────────────────────────────────────────────────┤
│ 2    │ Anthropic → Gemini │ 无法将 Anthropic 请求发送到 Gemini 上游        │
└──────┴────────────────────┴────────────────────────────────────────────────┘

实现文件
- src-tauri/src/proxy/gemini_compat/mod.rs
- src-tauri/src/proxy/gemini_compat/request.rs
- src-tauri/src/proxy/gemini_compat/response.rs
- src-tauri/src/proxy/gemini_compat/stream.rs
- src-tauri/src/proxy/gemini_compat/tools.rs
- src-tauri/src/proxy/openai_compat.rs
- src-tauri/src/proxy/response.rs
- src-tauri/src/proxy/server.rs
- src-tauri/src/proxy/server_helpers.rs

支持的功能
- Gemini ↔ Responses 请求/响应/流式转换
- Gemini 路径 fallback 到 OpenAI Responses/Chat
- systemInstruction / tools / toolConfig 映射
- response_format ↔ responseMimeType/responseSchema 映射
- functionCall / functionResponse 映射
- 多模态图片 data URL / fileUri 映射
- usage 统计与 finishReason 映射

测试
- cargo test (src-tauri) 通过
