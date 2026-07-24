# 统一「添加上游」入口

## 决策
- 工具栏只保留一个「添加上游」按钮，去掉并列「添加账户」。
- 对标 sub2api：单入口 → 弹窗内类型卡片分流。

## 交互
1. 点击「添加上游」
2. kind 步两张卡片：
   - API Key 上游 → 关闭统一弹窗，打开既有 Upstream 编辑器
   - 账户登录/导入 → 进入 Kiro/Codex/xAI 面板（可返回 kind）
3. 账户成功后仍 `onConfigReload`，后端 reconcile 出 account-backed Upstream。

## 关键文件
- `src/features/config/cards/upstreams/table.tsx` — toolbar 单按钮
- `src/features/config/cards/upstreams/add-account-dialog.tsx` — kind + account 两步
- `src/features/config/cards/upstreams-card.tsx` — 接线
- `messages/{zh,en}.json` — `upstreams_add_kind_*`、`common_back`
- 测试：`upstreams-account-ui.test.tsx`

## 验证
`pnpm exec vitest run src/features/config/cards/upstreams/upstreams-account-ui.test.tsx src/features/config/cards/upstreams/table.test.tsx` 全过。
