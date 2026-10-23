# Sub2API / CPA 更新对比与实施方案

核验日期：2026-10-23。状态：**用户确认的三个批次已完成本地实现和回归；未提交、推送、发布或调用真实供应商**。

## 实施结果

- Claude/Gemini 转换流缺终态、流内错误和 Gemini MALFORMED_FUNCTION_CALL 现在明确失败；保留已观测用量，已输出后不重放。顺带修复 Chat→Gemini 的 EOF 假 STOP。
- **更正调研结论**：Chat→Responses 的 `push_done` 原本已经检查 finish_reason，不存在最初表格推断的无条件成功；这条链路只补空流回归，没有重复改造。
- response.metadata 保留首输出前的恢复窗口；Claude 上下文超限正确映射 incomplete/length；Chat→Codex 保留显式并行 false。
- 结构化会话冻结拒绝在 HTTP 与 SSE 路径终止请求，不改冷却、不触发后备上游。现有普通认证错误处理保留。
- Codex 加密历史仅在明确错误、唯一定位、未输出时移除被拒绝的一项；HTTP/SSE 共用一次恢复预算，发送前对照实际 Authorization。凭据发生变化中止恢复；恢复后仍失败不会继续通用重试。原 xAI 专属恢复保留。
- Gemini Schema 先展开本地引用再清理，循环、远端、缺失、约束冲突明确拒绝；深度 64、节点 16,384、字符串/键累计 8 MiB 预算。工具结果图片经 Responses/Anthropic→Chat→Gemini 链路验证，保留为独立媒体 part。
- 成功原生 Responses JSON/SSE 补缺失/null created_at，同流回退值稳定，保留已有值；改写 JSON 删除旧 Content-Length，socket 测试验证完整收包。
- Codex 搜索历史声明在最终发送路径协调，兼容 Lite；新声明限制为 cached-only，保留原工具调用权限，不增加联网授权。
- Claude usage.speed 接入既有价格档位及转换后的 Responses service_tier，稀疏尾 usage 不清掉已有档位；flat reasoning_tokens 仅作嵌套字段缺失时的回退，不重复计输出。

验证：`cargo test --workspace` **1,311 项通过**（runtime 819、protocol 101、storage 99，其余 292）；`cargo fmt --all -- --check`、`git diff --check` 通过。定向测试包含真实 router/socket 的失败恢复次数、凭据变更阻止恢复、会话拒绝零后备调用、完整 HTTP framing、SQLite 首输出日志。此次未改前端，没有前端测试要求；模拟上游结果不等价真实供应商验收。

以下保留原方案与对比依据；“现状”描述调研基线，以上实施结果为当前工作区状态。

## 基线与范围

- 本项目：`747b017` / v0.1.221，调研开始时工作区干净。
- CPA：`router-for-me/CLIProxyAPI`，从上次专项审计 v8.0.17 到 [v8.0.40](https://github.com/router-for-me/CLIProxyAPI/releases/tag/v8.0.40)，23 个版本、273 个提交。已分页取得完整提交清单并筛查发行说明，重点项核对标签源码；不是全部提交的逐行审计。GitHub compare 默认只返回 250 个提交/最多 300 个文件，不能作为完整差异。
- Sub2API：按用户要求主要看 [ranxi v2.16.0](https://github.com/ranxi2001/sub2api/releases/tag/v2.16.0) / `5f779474d`，重点筛查 v2.12.2 之后的更新。这里的 v2.12.2 是本次调研窗口，不是 Token Proxy 已完整对齐的版本承诺。
- [Wei-Shaw v0.2.18](https://github.com/Wei-Shaw/sub2api/releases/tag/v0.2.18) 仅作补充来源；没有将其全部修复说成 ranxi v2.16.0 已包含。
- 这是 Rust 项目的选择性移植，不合并 Go 项目源码、不照搬账号调度或管理数据库。

## 第一批：请求语义、失败终态与错误分类

目标：优先消除假成功、错误重试和请求参数被覆盖。建议先单独完成、验证此批。

| 项目 | 已确认现状 | 实施范围与目标 | 验收 |
| --- | --- | --- | --- |
| 转换流提前结束 / error 事件 | Claude/Gemini EOF 分支存在假成功；Anthropic error 事件未显式处理。原生 Responses、Chat→Responses 已有截断检测（实施时复核更正） | 各转换器按自身协议追踪终态；未见终态的 EOF/error 生成协议内失败，保留已观测 usage 并记失败。参考 CPA `4f2e5270`，原版 Sub2API `4355236e0` | 无内容 EOF、部分正文 EOF、error SSE、transport error、正常终态、尾 usage；已输出内容绝不重放 |
| `response.metadata` 误触发放行 | `token_proxy_protocol/src/responses_error/prelude.rs:116` 缓冲 `codex.response.metadata`，但缺 `response.metadata`；后者落到 ReadyForPassThrough | 纳入元数据缓冲；同步检查 streaming.rs:1029 的首输出识别。参考 CPA `9e79552f`、上游 `codex_executor_terminal.go:516` | metadata 后 overload 仍在原有预算内恢复；metadata 不算正文/TTFT；真正正文后不能重试 |
| 会话级策略拒绝误伤 Upstream | `proxy/upstream/result.rs:40,149` 和 `account_selector/mod.rs:180` 会让 403 进入普通重试/冷却 | 精确识别可信结构化 `session_blocked_by_cyber_policy`，终止本次请求，不切上游、不改会话、不处罚 Upstream。参考 ranxi `ae3c2e699` | HTTP 403 与 HTTP 200 + response.failed；后备上游零调用、冷却不变；普通鉴权 403 行为不变 |
| 显式禁止并行被覆盖 | `proxy/codex_compat/request.rs:132` 在 Chat→Codex 无条件写 true；原生 Responses 路径已有保留逻辑 | 尊重显式 false，仅缺省时使用现有默认。参考 CPA `99d1698f` | true/false/缺失/null；保留 Responses Lite 既有约束 |
| Claude 上下文超限被当成功 | `token_proxy_protocol/src/compat_reason.rs:145` 未映射 `model_context_window_exceeded` | 转 Responses 为 incomplete/max_output_tokens，转 Chat 为 length；不改现有 pause_turn 合同。参考 CPA `d495e24c`、`c2dfec28` | 流式/非流式一致；普通 end_turn、refusal、pause_turn 回归 |
| Gemini 工具生成错误被当 stop | `proxy/gemini_compat/response.rs:292`、`stream.rs:645` 未专门处理 MALFORMED_FUNCTION_CALL | 明确失败；只在尚无业务输出且现有策略允许时重试。参考 CPA `7f2f17d7` | 无输出错误、带部分工具输出错误、正常 STOP/MAX_TOKENS，不伪造成功 |

表中的 `proxy/` 指 `crates/token_proxy_runtime/src/proxy/`。

## 第二批：工具与 Responses 兼容

| 项目 | 修改落点 | 边界与验收 |
| --- | --- | --- |
| Gemini Schema 本地引用与新关键词 | `crates/token_proxy_protocol/src/gemini_tools/schema.rs:5,67,114`。当前删除 `$ref/$defs` 而不展开，且缺 readOnly/writeOnly、contentMediaType/contentEncoding、discriminator、dependent* 清理。参考 CPA `c558d320` 等 | 先解析本地引用再清理；有深度/展开预算，循环和无法表达的引用明确处理；不抓远端 URL，不改 default/enum/const 用户值。测试 JSON pointer 逃逸、组合 Schema、同名属性、大整数 |
| Codex 加密 reasoning 精确恢复 | `proxy/upstream/request_repair.rs` 与 Responses prelude/dispatch；参考 ranxi `5c65680aa`、`9c0c4194a` | 当前已有 xAI 专用 HTTP400 修复，不是 Codex 的等价支持。新增独立 Codex 分类，仅明确失效且唯一可定位、未输出、未取消时同一 Upstream/凭据恢复一次；HTTP 与 SSE 共用预算。不得全删 reasoning，不删除 compaction；歧义保留失败。全局账号亲和性重构不纳入本批 |
| 原生成功 Responses 的 created_at | `token_proxy_protocol/src/responses_failure.rs:73` 当前只修失败，成功原生路径存在缺口；参考 ranxi `af8cf7f5f` | 补缺失/null，保留有效原值；同流采用同一回退时间，覆盖 JSON、created/in_progress/completed/incomplete；不伪称真实上游生成时间 |
| web_search 历史声明一致性 | `proxy/codex_compat/request.rs:186`；参考 ranxi `openai_oauth_web_search_history.go`、CPA `e7d4c731` | 本地支持部分搜索转换，但缺历史回放与声明协调。仅对需要该合同的 Codex 路径补最小 cached-only 声明，不扩大当前工具权限；无历史不注入、已有声明不重复、Lite additional_tools 一致。此项是顺带发现的既有缺口，不全是 ranxi 最近版本新增 |
| Gemini 工具结果中的图片 | `proxy/gemini_compat/request.rs:273,1122` 为候选缺口 | 先用 Responses/Anthropic→Chat→Gemini 的真实形状夹具确认；确实只进入 functionResponse JSON 而非媒体 parts 后再修。普通图片输入已支持不代表工具结果图片已支持 |

## 第三批：用量准确性

- Claude `usage.speed`：当前 `token_proxy_storage/src/usage/mod.rs:258,338` 主要解析 service_tier，未接入 fast/standard；接入已有价格档位机制，避免重建计费体系。实际响应档位优先；后续稀疏 usage 不得清掉前值。参考 CPA `b90190db`。
- flat `usage.reasoning_tokens`：`token_proxy_protocol/src/openai_usage.rs:124` 已支持标准嵌套格式，但缺 flat 回退；嵌套优先、缺失才回退，不重复计入总输出。参考 CPA `e198f3cd`。
- 客户端取消后是否排空上游：本地已有 Drop 保存已观测 usage，不能称完全漏记。默认本轮保留取消行为；若需要最终供应商 usage，再单独设计有时长/空闲预算的排空策略，避免取消后无界消费。

## 已有能力与本轮不移植项

已存在：原生 Responses EOF 失败、有界 SSE/请求体、完整响应详情落库、迟到成功不清新冷却、凭据刷新并发保护、Responses→Claude 并行开关、Gemini 函数签名往返、尾部 usage。只对受影响边界回归，不重做。

ranxi 的 SIWC、2FA 自动准入、IP/用户风控管理、证据加密、定时调度、质量/延迟评分，以及 CPA 的插件/Home 集群、新 Provider、完整多族签名框架，作为独立产品需求评估。本项目未发现普通 API Key 被 plan_type 自动改为订阅账号的同类路径，不照搬该账号分类补丁。

上游工具映射缓存优化先做本地 profile；不从 Go 项目的性能改动直接推断 Rust 存在相同瓶颈。保持本项目 Upstream 为调度单元、全局优先级和 convert_from_map 合同。

## 实施与验证顺序

1. 第一批先写能重现错误的行为测试，再作最小修改；终态/首输出和会话拒绝分开提交，方便审查与回退。
2. 第二批逐项落地，Schema、加密恢复各自独立；加密恢复需请求次数和凭据一致性断言，不能只断言最终 HTTP200。
3. 第三批用量 fixture 覆盖流式/非流式与稀疏尾 usage，再检查价格档位选择结果。
4. 定向运行 `token_proxy_protocol`、`token_proxy_runtime`、`token_proxy_storage` 相关测试；完成批次后 workspace 回归、fmt、diff 检查。使用现有真实 router/socket + 模拟上游验证重试次数、终态、usage、SQLite日志和冷却。
5. 不涉及前端，不增加新框架或数据库迁移；若实施时发现必需的领域契约变化，补 GLOSSARY/ADR 并重新说明范围。
6. 本地验证与正式发布/部署、真实供应商验收分开报告。用户随后确认三个批次全部实施；当前完成本地验证，没有发布部署。

## 证据范围

详细核查表保存在 `.planning/2026-10-23-upstream-update-audit-2026-10-23/cpa-protocol.md`、`sub2api-findings.md`；上游标签快照在 `/tmp/token-proxy-cpa-audit-20261023` 和 `/tmp/token-proxy-sub2api-audit-20261023`，属于本机会话临时材料。本文提供永久 release/commit 标识便于复核。
