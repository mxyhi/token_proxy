# Notes: Chat ↔ Claude Messages（对齐 new-api）调研摘记

## Sources

### Source: QuantumNous/new-api
- 关键文件/函数：
  - OpenAI Chat → Claude Messages（请求体）：`relay/channel/claude/relay-claude.go` `RequestOpenAI2ClaudeMessage`
  - Claude stream → OpenAI Chat stream：`relay/channel/claude/relay-claude.go` `StreamResponseClaude2OpenAI`
  - OpenAI Chat stream → Claude stream：`service/convert.go` `StreamResponseOpenAI2Claude`
  - tool_choice 映射：`relay/channel/claude/relay-claude.go` `mapToolChoice`
- 关键语义：
  - system：new-api 会累积多个 system，输出为 Claude `system` 数组 blocks（更通用）
  - tool / tool_calls：
    - Chat role=tool → Claude `tool_result`（通常挂在 user message content 里）
    - assistant.tool_calls → Claude `tool_use`
  - parallel_tool_calls ↔ disable_parallel_tool_use（取反）
  - image_url：URL 或 data URL → Claude image(base64)（必要时下载并 base64）

## Current Repo Findings (token_proxy)
- 已有转换链：
  - Chat ↔ Responses：`src-tauri/src/proxy/openai_compat.rs`
  - Responses ↔ Anthropic(Messages)：`src-tauri/src/proxy/anthropic_compat/*`
  - 流式：
    - Chat→Responses：`src-tauri/src/proxy/response/chat_to_responses.rs`
    - Responses→Chat：`src-tauri/src/proxy/response/responses_to_chat.rs`
    - Responses→Anthropic：`src-tauri/src/proxy/response/responses_to_anthropic.rs`
    - Anthropic→Responses：`src-tauri/src/proxy/response/anthropic_to_responses.rs`
- 已有路由 fallback：
  - `/v1/messages` 缺 anthropic 且有 openai-response：Claude↔Responses fallback
  - `/v1/responses` 缺 openai-response 且有 anthropic：Responses↔Claude fallback
- 缺口：
  - 没有 Chat ↔ Claude 直转或组合 fallback
  - Chat→Responses 请求体目前只是 `messages` 原样塞入 `input`，无法生成正确的 Claude `tool_use/tool_result/image`
  - Responses→Chat 对多模态（image/file）目前会压缩为纯文本，导致 Claude→Chat 丢图
  - Responses→Anthropic 当前 `system` 输出是 string，需要改成数组 blocks 以对齐 new-api

