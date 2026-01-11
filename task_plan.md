# Task Plan: Tauri 更新升级功能（stable + beta）

## Goal
在 Token Proxy 中实现 Tauri v2 自动更新（仅稳定版），使用 GitHub Releases 作为发布源。

## Phases
- [x] Phase 1: 计划与方案确认
- [x] Phase 2: 调研与现状分析
- [x] Phase 3: 实现更新能力（Rust + 前端）
- [x] Phase 4: CI/发布链路对接
- [ ] Phase 5: 验证与交付

## Key Questions
1. 是否启用启动时自动检查更新？
2. 证书缺失情况下的用户提示与风险说明边界？

## Decisions Made
- 渠道策略: 仅稳定版更新（不提供 beta 更新）。
- 发布源: GitHub Releases latest.json。

## Errors Encountered
- 无

## Status
**Currently in Phase 5** - 等待验证与交付
