# Token Proxy

Token Proxy 统一不同 AI Provider 的请求、响应、用量与计费语义，使 Dashboard 和成本统计使用同一套领域语言。

## Language

**Total Input**:
一次请求处理的全部输入 token，包括未缓存输入、缓存读取、缓存写入和图像输入。
_Avoid_: Prompt tokens（Provider 口径不一致）

**Cache Read**:
从既有提示缓存中复用的输入 token；只有 Cache Read 才构成缓存命中。
_Avoid_: Cached tokens（可能混入缓存写入）

**Cache Write**:
写入或创建提示缓存的输入 token，包括普通、5 分钟和 1 小时缓存写入；它属于输入，但不是缓存命中。
_Avoid_: Cache hit, cached input

**Cache Hit Rate**:
Cache Read 占 Total Input 的比例。
_Avoid_: Cache activity rate

**Usage Breakdown**:
将 Provider 原始用量拆成未缓存输入、Cache Read、各类 Cache Write、输出和图像 token 的规范化用量。
_Avoid_: Cached total

**Cached Creation Tokens**:
OpenAI-compatible usage 中单独表示 Cache Write 的输入 token 明细；它计入 Total Input，但不计入 Cache Read，也不构成缓存命中。
_Avoid_: Cached tokens、Cache Hit

**Reasoning Effort**:
一次模型请求期望使用的推理计算强度；它不决定推理摘要是否对客户端可见。
_Avoid_: Summary Visibility、Thinking Display

**Summary Visibility**:
推理摘要的跨 Provider 可见性意图，只有 Enabled、Disabled、Unspecified 三态；Unspecified 不得由 Reasoning Effort 自动推导为 Enabled。
_Avoid_: Reasoning Effort、Thinking Budget

**Error Request**:
最终 HTTP 状态码大于等于 400 的请求记录；它不参与长期请求统计，保留期（7 天）结束后整条删除。
_Avoid_: 仅以 response_error 是否存在判断错误请求

**Request Detail**:
为临时排障捕获的请求头、请求体、响应体和客户端 IP；不包含请求统计字段、`usage_json` 或错误摘要。成功请求的 Request Detail 在 7 天后清空，日志行本身永久保留。
_Avoid_: Request Log（请求日志整行）、Usage Breakdown 原始 JSON

**Success Request Log**:
`status < 400` 的请求日志行；永久保留，用于长期用量与成本统计。可清空 Request Detail，但不得删除整行，也不得清空 `usage_json` 与规范化 token/成本字段。
_Avoid_: 成功日志 90 天过期删除

**可用模型白名单**:
单个上游声明可以接收的入站模型集合。未配置或集合为空表示不限制模型；非空时仅允许精确匹配的模型参与该上游路由。
_Avoid_: 模型列表（容易与上游探测结果混淆）、模型映射

**模型映射**:
将客户端请求中的模型名改写为目标上游模型名的规则。它只负责改名，不决定模型能否路由到该上游。
_Avoid_: 可用模型、模型白名单

**Curated Pricing Overlay（官方精选计价层）**:
项目依据模型厂商官方公开价格维护、叠加在基础计价目录之上的小型模型价格集合；每个条目必须保留独立可追溯的官方来源，用户自定义计价仍可覆盖或删除其中条目。
_Avoid_: 基础计价目录、sub2api 快照、用户自定义计价、模型映射

**Same-Upstream Retry（原地重试）**:
可重试失败后，在切换到其它上游之前，对同一上游额外再发的次数；由全局配置 `same_upstream_retry_count` 控制，默认 1，0 表示关闭。
_Avoid_: 跨上游 failover、冷却

**Upstream（上游）**:
全局 routing/retry 策略作用的唯一候选/调度单元；单条 Upstream 自身承载 provider 能力、priority、proxy、enabled、模型限制/映射与 credential 等已实现字段（`same_upstream_retry_count` / `upstream_strategy` 为全局配置，非 per-upstream 字段）。
_Avoid_: Provider 条目、独立账户路由项、Accounts 池中的可调度对象、per-upstream retry/dispatch/order

**Provider Account / 账户凭据身份**:
持久化认证身份（OAuth token、Agent Identity 等），供 Account-backed Upstream 引用；不再独立承载 priority、proxy 或 enabled。
_Avoid_: 可调度账户、账户优先级、账户代理、账户开关

**Account-backed Upstream（账户型上游）**:
绑定一个 Kiro / Codex / xAI Provider Account 的 Upstream；一个 account 对应一条稳定 Upstream，credential identity 与 routing fields 分离。
_Avoid_: 前端构造的 default upstream、多 Upstream 共享同一 account、账户侧 priority/proxy/enabled

**Request Repair Retry（请求修复重试）**:
上游明确拒绝某个可安全移除的请求字段后，代理保持同一上游身份、仅删除该字段并再次发送；它修改请求，不计入 Same-Upstream Retry 次数。
_Avoid_: 原地重试、跨上游 failover、任意 400 重试

**Retry Scope（重试范围）**:
单次可重试失败允许的后续路由范围；`SameThenNext` 允许先原地再跨上游，`NextOnly` 跳过原地直接跨上游。它描述失败后的路由边界，不是重试次数。
_Avoid_: Retry Count、Cooldown Scope

**Upstream Attempt（上游 Attempt）**:
一次实际发往某个上游或账户的发送及其响应记录。每个 attempt 保留原始 usage、token、成本、账户和状态，用于排障与上游消耗审计；它不等于客户端看到的一次请求。
_Avoid_: 客户端请求账单、最终请求

**Final Client Request Billing（客户端最终请求账单）**:
同一入站请求经过重试或 failover 后，Dashboard、成本和请求量统计使用的唯一账单记录。中间 attempt 仍保留，但 `is_billable=0`；代理按请求内单调的 attempt 完成序号选择最终 attempt 候选，不能用并发发送启动顺序代替完成顺序。
_Avoid_: 将所有 upstream attempt usage 相加作为客户端账单

**No Configured Credential（未配置凭据）**:
Provider 有路由配置，但没有任何可用账号或 API 凭据，返回 HTTP 502。它是本地上游配置缺失，不是客户端鉴权失败。
_Avoid_: 401、账号冷却

**All Accounts Cooling（全部账号冷却）**:
Provider 存在有效账号，但当前请求作用域内全部处于 cooldown，返回 HTTP 503；若配置了其它 provider fallback，仍可继续降级。
_Avoid_: 未配置账号、账号禁用

**Model Not Supported（模型不支持）**:
存在匹配入站协议的上游，但请求模型被所有上游的可用模型白名单排除，返回 HTTP 404。
_Avoid_: 上游未配置、上游暂时不可用

**Responses Stream Event**:
`/v1/responses` 流中的单个 JSON 生命周期事件。每个事件都必须携带单调递增的 `sequence_number`；错误终止事件也不例外。`[DONE]` 是流结束哨兵，不是事件，不编号。
_Avoid_: SSE Chunk（传输分块可能拆分或合并事件）

**Pre-stream Error Response**:
Responses 流尚未向客户端提交时返回的 HTTP 4xx/5xx JSON 错误。它必须保留真实 HTTP 状态，并满足 OpenAI `ErrorResponse` 的 `type/message/param/code` 字段合同。
_Avoid_: `response.failed`（只用于已经提交的 SSE 流）

**Request-Scoped Content Policy Rejection（请求级内容策略拒绝）**:
由当前请求的 prompt 或 media 触发的内容策略拒绝；更换账号不会改变结果，也不得影响账号 cooldown。
_Avoid_: 账号访问失败、未知 403

**Account Access Failure（账号访问失败）**:
由账号 suspension、disabled 或 subscription/entitlement 缺失导致的访问拒绝；它属于账号身份，可参与账号 failover 和 cooldown。
_Avoid_: 请求级内容策略拒绝

**Payment Required Account Failure（账户付费要求失败）**:
账户型上游返回 HTTP 402，表示当前账户的订阅、余额或付费资格不可用；它属于账号访问失败，跳过原地重试，参与跨上游 failover 和 cooldown。
_Avoid_: 客户端付费要求、普通请求校验失败、Same-Upstream Retry

**Unknown Forbidden（未知 403）**:
缺少足够结构化证据来判断作用域的 403；保持账号级失败语义，避免把真实账号封禁误判为单请求拒绝。
_Avoid_: 含糊的 Policy Violation

**Codex OAuth Account（Codex OAuth 账户）**:
持久化 access/refresh token、到期时间和自动刷新设置的 Codex 身份；上游使用 Bearer 鉴权，401 可触发一次 OAuth token refresh。
_Avoid_: Agent Identity、Codex API Key

**Codex Agent Identity Account（Codex Agent Identity 账户）**:
从官方 Codex `auth.json` 导入的独立凭据类型，持久化 runtime ID、PKCS#8 Ed25519 私钥与 task binding，不持久化或伪造 OAuth token，也没有 token 到期/自动刷新语义。
_Avoid_: OAuth 登录开关、access token 别名、应用内生成身份

**Codex Alpha Search（Codex Alpha 搜索）**:
Codex 账户提供的原生搜索能力；入站 `/v1/alpha/search` 只能路由到 Codex，并保持请求、响应和查询参数的原生合同。
_Avoid_: OpenAI-compatible 搜索、formatless fallback、Responses 格式转换

**AgentAssertion（Agent Assertion）**:
每次 Codex 请求根据 runtime ID、当前 task ID 和 UTC 时间动态签名生成的 `Authorization` 值；断言本身不得写入日志或持久化。
_Avoid_: Bearer token、静态 API Key

**Agent Task Binding（Agent Task 绑定）**:
Agent Identity 与服务端 task ID 的持久化绑定。缺失时按账户加锁注册；只有明确的 task-invalid 401 才允许重新注册并重放一次，普通 401 不注册 task，也不进入 OAuth refresh。
_Avoid_: Same-Upstream Retry、无限 401 重试、每请求注册

**In-stream Terminal Failure**:
Responses SSE 已以 HTTP 200 提交后用于终止流的 `response.failed` 事件。事件必须包含连续的 `sequence_number`，其 `response` 必须包含 `created_at`、`model` 和完整失败状态；请求日志仍记录真实 4xx/5xx 失败状态。
_Avoid_: HTTP Error Response（响应头提交后已无法更改 HTTP 状态）

**xAI API Key Upstream（xAI API Key 上游）**:
使用开发者 API Key 访问 xAI 官方 API 的普通上游；它没有账户生命周期，继续使用 OpenAI-compatible provider 配置。
_Avoid_: xAI 账户、Grok OAuth 账户

**xAI OAuth Account（xAI OAuth 账户）**:
通过 xAI OIDC device-code 或 refresh token 获得并持久化的 Grok CLI OAuth 身份；承载刷新与配额语义，调度字段属于绑定它的 Upstream。
_Avoid_: xAI API Key、CPA API Key、普通 OpenAI 上游、账户级 priority/proxy/enabled

**xAI CLI Gateway（xAI CLI 网关）**:
xAI OAuth 账户发送文本 Responses 请求并读取该身份实时模型目录的受信服务端点 `cli-chat-proxy.grok.com`；该模型目录不是普通 API Key 上游的标准合同。
_Avoid_: xAI 官方 API、OpenAI-compatible base URL

**xAI Official API（xAI 官方 API）**:
xAI OAuth 账户发送仓库已有图片、视频和 Responses Compact 请求的受信服务端点 `api.x.ai`；它与 CLI 网关使用同一账户身份，承接 CLI 网关明确不支持的端点能力。
_Avoid_: xAI CLI 网关、自定义 base URL

**xAI X Search Injection（xAI X Search 注入）**:
由用户显式启用的全局能力，为原生 xAI Responses 请求补充服务端搜索工具；默认关闭。启用后，服务端内部搜索子调用属于执行 trace，不是客户端待执行工具调用。
_Avoid_: 客户端显式工具声明、Codex Alpha Search、默认联网搜索
