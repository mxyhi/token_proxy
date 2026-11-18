# 上游更新对 Token Proxy 的适用性审计

核对日期：2026-11-17。

## 版本基线

- Codex 官方模型目录提交 `6c0cee502d215e5df2dd9a303cf2e252229c9e12` 新增 `gpt-6.2-sol`、`gpt-6.2-astra`，并把 `gpt-6.1-sol` 上下文窗口改为 500,000。
- Sub2API 最新正式版为 `v2.22.0`，本地 fork 当前基线为 `v2.21.3-941-g908106cde`。
- CPA（`router-for-me/CLIProxyAPI`）最新正式版为 `v8.0.77`，其最新提交包含 GPT-6.2 目录、嵌套 input_audio、PDF/tool-result 媒体和 Retry-After 处理。

## 已更新

- `codex-models.json` 同步官方 GPT-6.2 目录，保持 TP 的菜单排序和隐藏项规则。
- Codex 配置默认模型改为 `gpt-6.2-sol`，官方模型候选与 `openai/` 热映射补齐 GPT-6.2。
- 价格目录同步 Sub2API 价格快照提交 `6107cf17487fe5cb0bd179457ede6b1fe8f7271e`，并以 OpenAI 2026-11-17 价格页覆盖 GPT-6.2 价格。标准价为 Sol `$2/$10`、Astra `$8/$40` 每百万输入/输出 token，补充 Standard、Fast、Flex、Batch 和当前阶段的 Ultrafast 价卡。
- 增加 GPT-6.2 价格和别名回归测试。现有 Codex 目录、价格和配置映射测试全部通过。

## 对比结论

### Sub2API v2.21.4–v2.22.0

适合 TP 的部分是模型目录、OpenAI GPT-6.2 价格、Fast/Ultrafast 价格档位、工具发现和媒体/工具历史保留。目录与价格已同步。TP 已经转发 `service_tier`，也已经有请求用量中的服务档位和缓存写入统计。

Sub2API 的 409 工具发现恢复、WS 会话续接、账号代理池、Vertex 多区域、供应商目录、分组调度和运营后台属于网关业务能力。TP 没有对应的共享 Redis、账号池、管理后台或 Sub2API 计费/迁移边界，不应直接移植。

OpenAI 价格页现在区分 30 分钟和 12 小时缓存写入。TP 的成本结构仍使用通用 `cache_write`、`cache_write_5m`、`cache_write_1h` 字段，缺少 30m/12h 的独立持久化字段。当前请求协议尚未证明会稳定返回新字段，因此先保留现有计费边界，后续若真实 usage 出现独立字段再扩展 schema 和回填。

### CPA v8.0.49–v8.0.77

GPT-6.2 目录已同步。CPA 的 nested `input_audio`、PDF 文档提取、Gemini tool-result 媒体、SSE/Retry-After 和本地错误边界在 TP 已有对应转换或回归测试。

CPA 的 payload replace 规则、插件 ABI/usage attribution、Home/管理 API、Responses WebSocket/duplex、动态插件和其它 provider 专用功能在 TP 没有对应产品边界。无需移植。

## 验证

- `cargo test -p token_proxy_runtime codex_catalog --lib`：6 通过。
- `cargo test -p token_proxy_storage pricing --lib`：32 通过。
- `cargo test -p token_proxy_config hot_model_mappings --lib`：6 通过。
- 当前运行中的 9208 服务匿名访问 `/codex/models.json` 返回 401。使用本地授权读取到旧进程目录，说明本次文件改动需要重启/重新发布运行实例后才会生效。
