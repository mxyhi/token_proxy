# CPA v7.3.6 对比与 Token Proxy 更新方案

核验日期：2026-09-17。状态：已按用户“全部实施”完成本地实现与回归；未提交、推送、发布或部署。

## 对比基线

| 项目 | 已核验版本 / 提交 | 说明 |
| --- | --- | --- |
| Token Proxy 当前工作树 | v0.1.179 / `9e5c925` | 开始调研时工作区干净 |
| CPA 历史兼容基线 | v7.2.154 | 上次调研记录；不能据此认定本项目尚未覆盖后续修复 |
| CPA 最新正式版 | v7.3.6 / `8c664b2fede5c83b919be1df9b01057ec4e4c950` | GitHub 发布于 2026-09-17 05:53:32 UTC |
| CPA main | `8c664b2fede5c83b919be1df9b01057ec4e4c950` | 本次查询与正式版相同，无额外未发布增量 |

CPA 指 [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)，并非按旧目录名推定为 CLIProxyAPIPlus。相对 v7.2.154，新增 **12 个正式版本、242 个提交**：v7.2.155–v7.2.159、v7.3.0–v7.3.6。已分页读取完整提交清单，并检出正式标签源码核对重点功能。GitHub compare 首屏只有 100 个提交，文件列表最多 300 项，因此没有将首屏当作完整差异。

来源：[正式发行](https://github.com/router-for-me/CLIProxyAPI/releases/tag/v7.3.6)、[版本比较](https://github.com/router-for-me/CLIProxyAPI/compare/v7.2.154...v7.3.6)。

同期本项目 PR #316 处理账户模型读取和渠道 UI 命名；初查时 OPEN，交付前复查已进入远端 main（`dbbcf12b16ce57ab7b95413fa3d4cb6f8c22fc8c`）。当前工作树仍为上述基线。此方案聚焦协议转换，实施前应重新核对合入状态，保留并发改动。

## 更新项筛选

| 上游变化 | 本项目现状 | 建议 |
| --- | --- | --- |
| 孤立 function/custom tool output 降级为 user 内容 | Responses→Chat 缺少 call_id 时直接丢弃；有 ID 但没有调用时仍生成 tool 消息 | **首批：修复内容丢失与无效工具配对** |
| 多种推理输出字段兼容 | 主要读取字符串 reasoning_content，未覆盖 reasoning / reasoning_details；流式先处理正文后处理推理 | **首批：统一提取并修正同一事件内顺序** |
| Gemini Schema 标识关键字清理 | 已清理 $schema/$id 等，但遗漏旧 id、$anchor、$vocabulary、$dynamicRef、$dynamicAnchor | **首批：按 Schema 节点清理并保留用户数据** |
| namespace 工具名限长和冲突映射 | 已支持展平、历史改名和部分响应还原；通用展平直接拼接 namespace__name，未限长；Codex 有独立缩名逻辑 | **第二批：统一声明、历史、选择和响应身份映射** |
| 模型能力元数据贯穿转换、纯文本模型图片处理 | CPA 新增的是内部 ModelInfo，不是普通请求 JSON metadata；本项目转换多数依赖 model_hint | 第二批设计，先明确可信能力来源；不按模型名猜测，不默认丢图片 |
| Gemini thought tokens 与尾部 usage | 本项目已有 gemini_usage 和流式尾部 usage 回归 | 已有对应实现，不重复移植 |
| Anthropic 交错工具流整理、工具参数截断终态 | 已有 tool_order 与 tests_compat_updates 覆盖连续工具块和不完整参数 | 已有对应实现，实施时回归 |
| SSE CRLF 分片、首输出预读与时间预算 | 已有共享 SSE 解析及 ResponsesPreludeInspector 预算 | 已有对应实现，保留首输出后不可重放边界 |
| Responses tool_choice→Chat、工具 strict 默认值 | 已有 tools.rs 转换及 Codex allowed_tools 支持 | 已有主要实现；namespace 白名单映射纳入第二批 |
| Cloudflare 520–526 瞬时错误 | 当前 is_retryable_status 已覆盖全部 5xx，已有 524 fallback 测试 | 不增加重复状态码分支 |
| Codex schema dialect / Unicode pattern 清理 | 已有 tool_schema.rs；遍历 Schema 节点，保留 default 中用户数据 | 保留；Gemini 修复不能误套到 Codex |
| Codex turn-state、模型级配额、鉴权刷新、重试策略 | 本项目已有独立账号/Upstream 与重试领域约定 | 单独评估行为差异，不用 CPA 调度器替换现有全局优先级 |
| Devin、Meta 原生提供商及 OAuth | 涉及新凭据、协议、模型目录、生命周期，超出小范围兼容补齐 | 按实际使用需求立项 |
| 插件 ABI/商店、TUI、LAN discovery、管理面配额展示 | 主要是 CPA 产品/部署形态扩展 | 本轮不移植 |

“已有”表示源码和已有测试用例可对应，不代表与 CPA 完全等价，也不表示本轮重新执行了这些测试。242 个提交已做分类筛选，未声称逐行审计全部新增提供商和插件代码。

## 建议首批实施范围

### 1. 孤立工具结果保留

CPA 参考：[8c984672](https://github.com/router-for-me/CLIProxyAPI/commit/8c984672)，标签源码 `internal/translator/openai/openai/responses/openai_openai-responses_request.go` 的 function_call_output / custom_tool_call_output 分支。

本地落点：`crates/token_proxy_runtime/src/proxy/openai_compat/input.rs:233`。

触发示例：历史只有 `{"type":"function_call_output","name":"send_message_to_thread","output":"任务结果"}`，没有匹配的 assistant 调用。当前无 call_id 时直接返回 None；若带未知 call_id，则形成上游可能拒绝的孤立 tool 消息。经 Chat 中转的 Gemini 路径也受影响。

方案：在已有 Responses→Chat 历史转换中跟踪实际待配对调用。合法配对维持 tool 消息；无配对结果按原顺序保留为 user 内容，不伪造调用，不把 fco_ 项 ID 当 call_id。复用已有输出文本/多模态转换，不添加依赖。

验收：无 ID、未知 ID、重复输出、并行调用、custom 输出、合法配对、结构化/多模态结果均有明确行为。现有 `responses_request_to_chat_skips_tool_output_without_call_id` 固化了丢弃语义，需要随新合同修改。

### 2. 推理文本多字段兼容

CPA 参考：[77820cb2](https://github.com/router-for-me/CLIProxyAPI/commit/77820cb2)、[c8ecb4f3](https://github.com/router-for-me/CLIProxyAPI/commit/c8ecb4f3)，标签源码 `internal/translator/openai/claude/openai_claude_response.go`。

本地落点：`crates/token_proxy_runtime/src/proxy/openai_compat/mod.rs:1117` 与 `crates/token_proxy_runtime/src/proxy/response/chat_to_responses.rs:253`。

触发示例：Chat 上游只返回 `delta.reasoning`，或 `reasoning_details:[{"type":"reasoning.text","text":"..."}]`。当前转换无法提取这些推理正文；同一 delta 同时携带 content 和 reasoning_content 时，正文先被提交。

方案：复用一个小型文本提取函数，按非空值优先级 reasoning_content → reasoning → reasoning_details 读取字符串、数组中的明确文本；不叠加重复别名，不将签名/加密内容解释为文本。让缓冲及流式路径一致，同一事件先推理后正文，保留已有 thinking_blocks 和签名合同。

验收：流式和非流式结果一致；空/null 主字段能回退；多字段同时出现不重复；推理先于同一事件内正文；加密字段不泄漏；不改变 Summary Visibility 的三态语义。

### 3. Gemini Schema 标识清理

CPA 参考：[f668ac41](https://github.com/router-for-me/CLIProxyAPI/commit/f668ac41)，标签源码 `internal/util/gemini_schema.go`，及 `TestCleanJSONSchema_RemovesDraft04IdAndSchemaIdentifierKeywords`。

本地落点：`crates/token_proxy_protocol/src/gemini_tools.rs:5` 和 `clean_tool_schema_object`。

触发示例：MCP 工具参数中的 `properties.kind.id="ContentType"` 或 `$anchor` 仍被带入 Gemini functionDeclarations。

方案：补齐不支持关键字，但只清理 Schema 元数据节点；合法 `properties.id` 等属性名必须保留，default/enum/const 等用户值不参与关键字删除。现有遍历会递归进入普通值，因此不能只往黑名单加五个字符串。

验收：根层、嵌套、数组项、组合 Schema 清理；合法同名属性和默认值不变；大整数不失真；已有 Gemini Schema 回归继续通过。首批限定工具参数 Schema；结构化输出 responseSchema 的方言转换单独评估，不能直接套用工具 Schema 的有损转换。

## 第二批的边界

- 工具名：参考 CPA `e3e97ad9` 及 `internal/util/responses_tools.go`，在现有工具身份模块内处理稳定限长、字符清理和碰撞；同步 tools、input、tool_choice/allowed_tools 与输出还原。当前 `tool_identity.rs:183` 仅拼接，不能靠单独截断一处修复。
- 模型能力：参考 CPA `a9e92b81` 与 `c4982e84`，先定义“能力未知”与“明确纯文本”的区别，再决定图片和 native search 行为。避免新增一个未经需求确认的通用中间件框架。
- 新 Provider：Meta/Devin 需要单独确认目标账户类型与认证流程。可先通过现有 OpenAI-compatible 上游接 CPA，原生接入另立范围。
- 不迁移 CPA 的账号调度状态；维持本项目以 Upstream 为唯一调度单元、全局 priority、首输出后不重放和最终账单唯一记录的合同。

## 实施后的验证与文档要求

已按“孤立结果 → 推理字段 → Schema → 工具名 → 模型能力”完成实现和验证；每项沿用现有测试框架，覆盖上述真实故障输入。只记录转换分支和计数，不记录工具正文、推理正文或凭据。复杂逻辑按可读性拆分，不新增依赖。

验证结果：

- `cargo test -p token_proxy_protocol --lib`：71 项通过。
- `cargo test -p token_proxy_runtime --lib`：676 项通过。
- `cargo test -p token_proxy_config --lib`：89 项通过。
- `pnpm run i18n:compile` 与 `pnpm exec tsc --noEmit`：通过。
- `cargo fmt --all -- --check` 与 `git diff --check`：通过。

建议检查：

```sh
cargo test -p token_proxy_protocol --lib
cargo test -p token_proxy_runtime --lib
cargo fmt --all -- --check
git diff --check
```

运行时包回归包含现有流式转换/工具身份测试。若实现改变共享路径，再按实际依赖扩大验证。源码调研或本地测试均不能表述为真实提供商验收。

已在 CONTEXT.md 补充“孤立工具结果”和“模型能力覆盖”术语，并在 README 中记录 `overrides.model_capabilities` 配置格式；推理文本载体沿用现有 Reasoning/Carrier 术语。此次是可逆兼容修复，不额外创建 ADR；若第二批引入模型能力来源或新的路由决策，再评估 ADR。
