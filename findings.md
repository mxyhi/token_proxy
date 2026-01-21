# Findings & Decisions

## Requirements
- 新增独立“导入 KAM JSON”按钮
- 使用文件选择（非目录）
- 解析 Kiro-account-manager 的 AccountExportData JSON
- 将 accounts[].credentials 转为本项目 KiroTokenRecord

## Research Findings
- Kiro-account-manager 的导出结构在 `src/renderer/src/types/account.ts`：
  - `AccountExportData { version, exportedAt, accounts, groups, tags }`
  - `Account.credentials` 包含 `accessToken`、`refreshToken`、`clientId`、`clientSecret`、`region`、`startUrl`、`expiresAt`、`authMethod`、`provider` 等
- 本项目 Kiro token 结构在 `src-tauri/src/kiro/types.rs`（KiroTokenRecord）：
  - `access_token`、`refresh_token`、`expires_at`(RFC3339)、`auth_method` 等

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| 使用 dialog 选择 JSON 文件 | 符合“独立导入选项”与跨平台需求 |
| 仅支持 AccountExportData | 遵循“No backward compatibility” |
| auth_method 归一化映射 | 保证刷新逻辑可用（builder-id / idc / social） |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
|       |            |

## Resources
- `/tmp/kiro-account-manager/Kiro-account-manager/src/renderer/src/types/account.ts`
- `src-tauri/src/kiro/types.rs`
- `src-tauri/src/kiro/store.rs`
