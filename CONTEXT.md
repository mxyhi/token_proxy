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

**模型目录展示名**:
仅在模型目录中供客户端选择器显示的人类可读名称。稳定模型 ID 仍是请求路由、响应模型字段和计费查询的唯一键；展示名不得参与请求改写。
_Avoid_: 模型映射、响应模型名、上游模型 ID

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

**Responses Incomplete（Responses 未完成终态）**:
响应因 token 上限、长度限制或内容过滤而提前结束的终态；输出项保持 incomplete，客户端不得将其视为 completed 或伪造完整工具参数。
_Avoid_: 正常完成、传输中断、空工具参数自动完成

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

**Codex Remote Compaction v2（Codex 远程压缩 v2）**:
Codex OAuth 普通 Responses 请求上的远程上下文压缩能力；通过非空 beta capability 集合声明，并由 `compaction_trigger` 触发。公开 OpenAI compaction 仍使用普通 `/v1/responses` 的 `context_management`；legacy `/v1/responses/compact` 不受支持。
_Avoid_: 独立 compact endpoint、公开 OpenAI context management、客户端本地摘要

**Codex Turn-State Provenance（Codex Turn-State 来源）**:
`x-codex-turn-state` 是某个 Codex 账户为一个客户端会话铸造的不透明状态；已知来源时只能回送同一账户，跨账户 failover 必须删除。未知、无会话或已过期来源不建立归属。
_Avoid_: 全局会话状态、可解析业务数据、跨账户共享状态

**AgentAssertion（Agent Assertion）**:
每次 Codex 请求根据 runtime ID、当前 task ID 和 UTC 时间动态签名生成的 `Authorization` 值；断言本身不得写入日志或持久化。
_Avoid_: Bearer token、静态 API Key

**Agent Task Binding（Agent Task 绑定）**:
Agent Identity 与服务端 task ID 的持久化绑定。缺失时按账户加锁注册；只有明确的 task-invalid 401 才允许重新注册并重放一次，普通 401 不注册 task，也不进入 OAuth refresh。
_Avoid_: Same-Upstream Retry、无限 401 重试、每请求注册

**In-stream Terminal Failure**:
Responses SSE 已以 HTTP 200 提交后用于终止流的 `response.failed` 事件。事件必须包含连续的 `sequence_number`，其 `response` 必须包含 `created_at`、`model` 和完整失败状态；请求日志仍记录真实 4xx/5xx 失败状态。
_Avoid_: HTTP Error Response（响应头提交后已无法更改 HTTP 状态）

**Codex Message Item ID**:
Codex Responses 输入中 `type=message` 项的跨事件标识；非空值必须以 `msg` 开头，规范化时为不合规值添加 `msg_`，再执行长度压缩和去重。
_Avoid_: 任意 Responses item id、reasoning item id

**Codex Replayable Reasoning Item ID**:
`store=false` Responses 输入中可回放 reasoning 项的标识；只有携带有效 `encrypted_content` 的项可保留 ID，且非空 ID 使用 `rs` 前缀。
_Avoid_: 无载体 reasoning ID、message item ID

**Codex Function Call Item ID**:
Codex Responses 输入中 `type=function_call` 项的标识；非空值使用 `fc` 前缀。它与 `call_id` 及 function-call-output item ID 是不同身份。
_Avoid_: call_id、function-call-output item ID

**Codex Custom Tool Item ID**:
Codex Responses 输入中 custom tool 调用及输出项的标识；调用使用 `ctc` 前缀，输出使用 `ctco` 前缀，并在单次请求内确定、幂等且无碰撞。
_Avoid_: call_id、function-call item ID、随机 ID

**Custom Tool Call（自定义自由格式工具调用）**:
携带自由格式 `input` 的 Responses 工具调用项；它与输出项通过 `call_id` 配对，在仅接受对象参数的 Provider 协议中仍保留同一调用身份。
_Avoid_: Function Arguments Object、孤立 tool result

**Explicit Null Tool Schema Type（显式空工具 Schema 类型）**:
工具参数 schema 中明确声明的 `type: null`，它不是缺失类型，属于无效 schema；缺失 `type` 表示不约束类型，语义不同。
_Avoid_: 缺失 type、空 parameters、schema type array

**Gemini Hidden Thought（Gemini 隐藏思考）**:
Gemini 历史 part 中 `thought=true` 的内部推理内容；system instruction 与普通 contents 都不得把它转换为可见对话历史。
_Avoid_: 可见 assistant 文本、推理摘要、普通 thought signature

**Gemini Tool Pairing（Gemini 工具配对）**:
Gemini function call 与 function response 的稳定关联；优先保留显式 `id`、`call_id` 或 `callId`，缺失时确定性生成，并按函数名 FIFO 消费待配对调用。
_Avoid_: 仅按数组位置配对、同名调用复用 ID、随机 ID

**Ordered Content Block**:
Anthropic 消息内容中按原始 `content_block.index` 排列的单个 thinking、text 或 tool_use 项；Responses 转换必须以该顺序生成 added、delta、done 和最终 output。
_Avoid_: 单一 message 聚合、按类型合并

**Function Arguments Object**:
Responses function call 的最终参数 JSON 对象；即使上游没有发送 arguments delta，也必须输出 `{}`，不能输出空字符串。
_Avoid_: 空参数字符串、缺省参数

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
xAI OAuth 账户发送仓库已有图片和视频请求的受信服务端点 `api.x.ai`；它与 CLI 网关使用同一账户身份，承接 CLI 网关明确不支持的媒体端点能力。
_Avoid_: xAI CLI 网关、自定义 base URL

**xAI X Search Injection（xAI X Search 注入）**:
由用户显式启用的全局能力，为原生 xAI Responses 请求补充服务端搜索工具；默认关闭。启用后，服务端内部搜索子调用属于执行 trace，不是客户端待执行工具调用。
_Avoid_: 客户端显式工具声明、Codex Alpha Search、默认联网搜索

**Claude Reasoning Carrier（Claude 推理载体）**:
Responses reasoning item 的 \`encrypted_content\` 在 Claude 双向转换中的语义载体；普通值表示 thinking signature，\`claude-redacted-thinking:\` 前缀值表示必须原样回放的 \`redacted_thinking\` 数据。
_Avoid_: 将所有 encrypted content 都当作 redacted thinking、将 marker 当作 signature

**Claude Server-side Web Search（Claude 服务端搜索）**:
Claude 原生 \`server_tool_use\`/\`web_search_tool_result\` 内容块与 Responses \`web_search_call\` 的双向表示；搜索结果中的有效加密索引可成为文本引用，缺失或空结果按无结果降级。
_Avoid_: 把服务端搜索当作客户端 function tool、把无效搜索结果伪造成有效引用

**Responses Output Item ID Hydration（Responses 输出项 ID 补全）**:
当终态 \`response.output\` 项缺少 ID 时，依据同一 \`output_index\` 的 added/done 生命周期事件补回非空 ID；已有 ID 和无可靠来源的项保持不变。
_Avoid_: 按类型猜 ID、伪造 ID、覆盖已有 ID

**SSE Done Boundary（SSE Done 边界）**:
OpenAI-compatible SSE 中的 \`[DONE]\` 终止哨兵；它只输出一次，哨兵后的同批及后续 payload 不属于客户端响应。
_Avoid_: 把 \`[DONE]\` 当 JSON 事件、继续转发终止后的 payload
