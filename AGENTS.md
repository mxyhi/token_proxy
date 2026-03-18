# Token Proxy 代码库rule

本文件为在 `token_proxy` 项目中工作的 AI 智能体提供指导。

## 项目概览

**Token Proxy** 是基于 Tauri 的 AI API 代理工具，用于转发 OpenAI、Gemini、Anthropic 等 AI API 格式，支持本地运行、token 使用统计、负载均衡和优先级管理。

- 前端: React 19 + TypeScript + Vite + Tailwind CSS v4 + shadcn/ui(pnpm dlx shadcn@latest add xxx)
- 后端: Rust (Edition 2021) + Tokio + Axum
- 桌面框架: Tauri 2

## 参考项目

- 代理转发/转换参考[litellm](.reference/litellm)
- 代理转发/转换参考[new-api](.reference/new-api)
- kiro、codex、antigravity等2api参考[CLIProxyAPIPlus](.reference/CLIProxyAPIPlus)
- CLIProxyAPIPlus的可视化app参考[quotio](.reference/quotio)
