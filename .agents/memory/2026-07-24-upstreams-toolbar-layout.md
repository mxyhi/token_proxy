# Upstreams 工具栏布局

## 决策（2026-07-24）
- 左：策略控件（顺序/派发/…），label **横排**（对齐 dashboard 筛选条）
- 右：密钥显隐 → 列 → **添加上游**（主按钮 default + Plus 图标）
- Select 固定 `h-9 w-[120px]`，不再上下堆 label 导致与按钮高低错位

## 文件
- `src/features/config/cards/upstreams/table.tsx` — `UpstreamsToolbar`

## 反例
- 勿把添加/列塞策略左侧同一坨；上下 label 会很难看
