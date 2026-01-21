# Task Plan: KAM JSON 导入

## Goal
新增独立导入按钮，支持从 Kiro-account-manager 导出的 JSON（AccountExportData）导入到本项目的 Kiro 账号列表。

## Current Phase
Phase 5

## Phases

### Phase 1: Requirements & Discovery
- [x] Understand user intent（新增“独立导入选项”支持 KAM JSON）
- [x] Identify constraints（不做兼容；TS 禁止 any；异步 IO）
- [x] Document findings in findings.md
- **Status:** complete

### Phase 2: Planning & Structure
- [x] Define technical approach（前端文件选择 → 后端解析 AccountExportData）
- [x] Decide UI entry（独立按钮）
- [x] Document decisions with rationale
- **Status:** complete

### Phase 3: Implementation
- [x] Add new API + Tauri command for KAM JSON import
- [x] Parse AccountExportData.accounts[].credentials -> KiroTokenRecord
- [x] Add new import button in Kiro login dialog
- [x] Update i18n messages
- **Status:** complete

### Phase 4: Testing & Verification
- [x] Run/record tests if executed
- [x] Verify requirements met
- **Status:** complete

### Phase 5: Delivery
- [x] Review changes and summarize
- [x] Provide next steps
- **Status:** complete

## Key Questions
1. 导入入口形式？（独立按钮）
2. 支持的格式？（Kiro-account-manager 的 AccountExportData JSON）
3. 是否兼容其他格式？（否，按“No backward compatibility”）

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| 独立按钮导入 KAM JSON | 与现有目录导入分离，行为清晰 |
| 仅支持 AccountExportData | 遵循“不做兼容”，聚焦目标格式 |

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
|       | 1       |            |

## Notes
- Update phase status as you progress
- Log errors immediately
