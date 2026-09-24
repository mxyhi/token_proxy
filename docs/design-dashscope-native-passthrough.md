# 设计：DashScope 原生协议透传 provider（/v1/services 前缀匹配）

状态：待审核
日期：2026-09-24
分支：`feat/my-proxy-endpoint`（随其他自有增强一并维护）
关联研究笔记：`docs/aliyun-dashscope/dashscope-api-notes.md`（DashScope 三形态接口规范与官方来源）

## 1. 问题

用户需要在代理中直接使用阿里云百炼 DashScope **原生协议**端点（`/api/v1/services/aigc/...`、`/api/v1/services/embeddings/...`、`/api/v1/services/rerank/...`）。现状：

1. `/v1/services/*` 不匹配任何已知路由，落入 `resolve_formatless_plan` 按优先级随机挑 provider（`dispatch.rs:246`），出站 URL 被拼成 `base_url + /v1/services/...`，与 DashScope 真实域名结构不符，必然 404
2. GUI 提供商下拉没有 `dashscope` 选项，url_compose 面板没有 dashscope 家族

需求：新增一个 `dashscope` provider，像 anthropic 的 `/v1/messages` 一样把 `/v1/services` 作为它的**家族根路径**——只要入站请求命中 `/v1/services` 前缀就走 dashscope 上游透传；`services` 之后的子路径不枚举、不感知，出站 URL 完全由 dashscope 自己的 url_compose 配置决定。**最小入侵、与原软件兼容**。

## 2. 现状（事实，2026-09-24 调研）

| 事实 | 位置 |
| --- | --- |
| 分发主链：alpha search → models → responses compact → openai native → compatible_plans → gemini native → anthropic → **formatless 兜底** | `crates/token_proxy_runtime/src/proxy/server/dispatch.rs:346-384` |
| formatless 兜底按优先级挑 `[openai, openai-response, anthropic]`，不看路径 | `dispatch.rs:246-254` |
| 纯透传 = `DispatchPlan { provider, outbound_path: None, request_transform: None, response_transform: None }`（base_plan） | `dispatch.rs` 内既有 |
| `provider_upstreams()` 是按 provider 字符串查 map，**任意字符串 provider 天然可用**，无白名单 | `crates/token_proxy_config/src/lib.rs:89-91` |
| url_compose 家族解析按 provider 硬编码三分支（openai / openai-response+xai / anthropic） | `crates/token_proxy_config/src/my_url_compose/compose.rs:27-38` |
| url_compose 配置结构仅三家族；猜测式拼接修正已整体移除（纯拼接是明确约定） | `my_url_compose/config.rs:15-22`、`crates/token_proxy_config/src/types.rs:652-656` |
| GUI 下拉选项 = `DEFAULT_PROVIDER_OPTIONS` 四项 + 配置已有 provider 并入 | `src/features/config/cards/upstreams/constants.ts:55-60` |
| url-compose 面板家族数组、prefix 示例、默认后缀、子路径映射示例均为前端常量表 | `src/features/config/cards/upstreams/url-compose-editor.tsx:17-48` |
| 未知 provider 在下游统计 match 站点安全：`crates/token_proxy_runtime/src/proxy/response/token_count.rs:16` 无分支静默跳过（`_ => {}`）；`crates/token_proxy_runtime/src/proxy/response/streaming.rs:868` 有 `_ =>` default | 已逐一验证 |
| DashScope 原生端点域名结构：`https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api/v1/services/...` | 研究笔记 §1 |

## 3. 目标与非目标

### 目标

- `dashscope` 出现在上游编辑器的提供商多选框，可勾选
- 勾选后 url_compose 面板出现 dashscope 独立家族（prefix/suffix 两窗口），prefix 提示示例为 **`/api`**
- 入站 `POST /v1/services/*` 且存在勾选 dashscope 的上游 → 选中该 provider，**纯透传**（body、子路径、query、其余请求头原样；仅按现有机制替换 Authorization）
- 出站 URL 由 url_compose.dashscope 配置纯拼接得出
- 前端拼接输入窗口空态提示字号缩小，防溢出
- 全程零回归：不勾选 dashscope 时行为与今天完全一致

### 非目标（本版不做）

- 不做任何格式转换（无 FormatTransform、无 SSE 重排、无 usage 键名映射）
- 不注入 `X-DashScope-SSE` 等请求头（客户端自带）
- 不把 dashscope 加入 formatless 兜底候选池 / compatible_plans / retry fallback 链
- 不枚举 `/v1/services` 子路径，不做 `/api/v2/apps` 等其他前缀（将来按同模式加常量即可）
- 不解析 DashScope 原生响应的 usage（统计面：这些请求按透传流量呈现；后续增强另立设计）
- 不复活任何"猜测式"URL 修正

## 4. 关键决策

### 决策 A — 未命中兜底：静默穿透（已确认）

命中 `/v1/services` 前缀但未配置 dashscope 上游时，判定函数返回 `None`，链条继续走现有 formatless 兜底——与今天行为逐字节一致，零回归。**不新增错误分支**。

### 决策 B — 匹配语义：前缀精确命中，唯一入口

`dashscope` 仅由 `/v1/services` 前缀命中（常量 `DASHSCOPE_SERVICES_ROOT`）。前缀判定对齐 `is_anthropic_path`（`proxy/server_helpers/mod.rs:49`）惯例：恰为 `/v1/services` 或其后随 `/`，`/v1/servicesX` 不误命中。不进 formatless 候选池，意味着它**永远不会**被分配到其他未知路径；`/v1/services` 永远不会被兜底错拿（只要存在 dashscope 上游）。

### 决策 C — 挂载位置：独立判定步骤（方案 1）

在 `resolve_dispatch_plan_with_request` 的 `resolve_anthropic_plan` 之后、`resolve_formatless_plan` 之前插入 `resolve_dashscope_native_plan`。与 anthropic/gemini 判定平级，语义独立；否决"改造 formatless 函数使其看路径"（污染语义）与"数据驱动通用前缀映射"（为一个 provider 引入新配置面，YAGNI）。

### 决策 D — 透传即 base_plan

复用现有 `base_plan(provider)`：`outbound_path: None` → `resolve_outbound_path` 走 `(None, _) => path.to_string()` 原样返回（`dispatch.rs:413`）；URL 组合交给 `compose_outbound_url`（provider="dashscope" → 新家族）。版本段替换天然工作：入站第一段 `/v1` 被 suffix 版本段替换，`/v10/...` 等不受影响（`compose.rs:76-85`）。

### 决策 E — GUI 细节（用户指定）

- `PREFIX_EXAMPLE.dashscope = "/api"`（prefix 窗口提示示例）
- `DEFAULT_SUFFIX.dashscope = "/v1/services/aigc/text-generation/generation"`（新建预填代表端点，与 openai 系预填机制一致）
- 拼接输入窗口空态提示：Input 加 `placeholder:text-[10px]`（当前继承 `text-xs`），缩小字号防溢出

### 决策 F — 补丁标记与维护规范

按仓库既有 `══════════ MY-XXX PATCH N (part) START/END ══════════` 约定，本特性统一命名 **`MY-DASHSCOPE-PASSTHROUGH`**，逐文件编号（见 §5），保证上游 pull 后可按标记重放注入（与 my-url-compose 等模块同一维护方式）。

## 5. 设计方案

### 5.1 后端 — 4 处增量（Rust）

**`crates/token_proxy_runtime/src/proxy/server/mod.rs`**

```rust
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 1 (const) START ══════════
const PROVIDER_DASHSCOPE: &str = "dashscope";
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 1 (const) END ══════════
```

（与 PROVIDER_ANTHROPIC 等常量并排；模块私有 `const` 即可——Rust 子模块天然可见父模块私有项，dispatch.rs 现正这样使用 PROVIDER_ANTHROPIC。）

**`crates/token_proxy_runtime/src/proxy/server/dispatch.rs`**

```rust
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 1 (dispatch) START ══════════
const DASHSCOPE_SERVICES_ROOT: &str = "/v1/services";

/// DashScope 原生协议透传：/v1/services 前缀命中且配置了 dashscope 上游才接管，
/// 否则返回 None 静默穿透（保持既有 formatless 行为，决策 A/B/C）。
fn resolve_dashscope_native_plan(
    config: &ProxyConfig,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    // 前缀精确命中：恰为 /v1/services 或其后随 '/'（对齐 is_anthropic_path 惯例）。
    let prefix_matched = path == DASHSCOPE_SERVICES_ROOT
        || (path.starts_with(DASHSCOPE_SERVICES_ROOT)
            && path.as_bytes()[DASHSCOPE_SERVICES_ROOT.len()] == b'/');
    if !prefix_matched {
        return None;
    }
    if config.provider_upstreams(PROVIDER_DASHSCOPE).is_none() {
        return None;
    }
    let provider = choose_provider_by_priority(config, None, &[PROVIDER_DASHSCOPE])
        .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
    return Some(provider.map(base_plan));
}
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 1 (dispatch) END ══════════
```

调用点（`resolve_dispatch_plan_with_request` 内，`resolve_anthropic_plan` 之后）：

```rust
    if let Some(plan) = resolve_dashscope_native_plan(config, path) {
        return plan;
    }
    resolve_formatless_plan(config)
```

不动：formatless 候选数组、`resolve_retry_fallback_provider`（dashscope 落 `_ => None`，无格式回退，符合预期）、`compatible_plans`。

**`crates/token_proxy_config/src/my_url_compose/config.rs`**

```rust
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) START ══════════
// UrlComposeConfig 增加字段
#[serde(default, skip_serializing_if = "Option::is_none")]
pub dashscope: Option<EndpointCompose>,
// normalized() / is_empty() 各补一行对应分支
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) END ══════════
```

**`crates/token_proxy_config/src/my_url_compose/compose.rs`**

```rust
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (compose) START ══════════
"dashscope" => compose.dashscope.as_ref(),
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (compose) END ══════════
```

（`family_compose` match 内一行。）

### 5.2 前端 — 4 个文件 + i18n

**`src/features/config/types.ts`**（PATCH 3 (types)）：`UrlComposeConfig` 增加 `dashscope?: UrlComposeEndpoint`。

**`src/features/config/cards/upstreams/constants.ts`**（PATCH 3 (options)）：`DEFAULT_PROVIDER_OPTIONS` 增加 `"dashscope"` → 多选框出现选项。

**`src/features/config/cards/upstreams/url-compose-editor.tsx`**（PATCH 3 (editor)）：

```ts
const FAMILIES: readonly UrlComposeFamily[] = ["openai", "openai-response", "anthropic", "dashscope"];

const DEFAULT_SUFFIX = { /* 既有三项 */ dashscope: "/v1/services/aigc/text-generation/generation" };
const PREFIX_EXAMPLE = { /* 既有三项 */ dashscope: "/api" };          // 决策 E
const MAP_EXAMPLES = { /* 既有三项 */
  dashscope: [
    "/v1/services/aigc/text-generation/generation",
    "/v1/services/aigc/multimodal-generation/generation",
    "/v1/services/embeddings/text-embedding/text-embedding",
    "/v1/services/rerank/text-rerank/text-rerank",
  ],
};
```

- `FAMILY_LABEL`/`FAMILY_TAG` 走新增 i18n key（§5.3）
- prefix/suffix 两类 Input 的 className 追加 `placeholder:text-[10px]`（空态提示缩小防溢出，决策 E）
- 仿 gemini 提示模式（`url-compose-editor.tsx:375-379`），勾选 dashscope 时显示一行说明：dashscope 家族仅接管 `/v1/services` 前缀的透传请求

**`messages/zh.json` / `messages/en.json`**（PATCH 3 (i18n)）：按既有 key 模式新增 `url_compose_family_dashscope`、`url_compose_family_dashscope_tag`、`url_compose_dashscope_hint`。其余复用既有 key。

### 5.3 数据流（验证用例）

勾选 dashscope，上游 `base_url = https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com`，url_compose.dashscope = `{ prefix: "/api", suffix: "/v1/services/aigc/text-generation/generation" }`：

```text
入站  POST /v1/services/aigc/multimodal-generation/generation
判定  resolve_dashscope_native_plan：前缀命中 + 存在 dashscope 上游 → base_plan("dashscope")
出站  compose_outbound_url：base + "/api" + 版本段替换(入站首段 /v1 → /v1) + 其余路径
    = https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation
认证  Authorization 替换为上游凭据（既有机制）；body/query/其余头原样
```

embedding / rerank 端点仅换子路径，同一套配置直接工作——"变化很大"的端点差异全部被配置吸收。

### 5.4 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 未知 provider "dashscope" 在下游硬 match 站点 panic | 已审计两大站点安全（token_count 无分支跳过、streaming 有 default）；实现期再全局 grep `match provider` / provider 字符串站点复核一遍 default 分支 |
| `/v1/services` 被其他兼容服务用作同名前缀 | 仅当配置了 dashscope 上游才接管（决策 A）；未配置行为不变 |
| 流式请求缺 `X-DashScope-SSE` 头导致非流式返回 | 透传契约：客户端按 DashScope 原生规范自带；GUI hint 说明中提示 |
| 透传流量统计缺 usage | 明确为非目标（决策边界）；后续如需，另立 dashscope_usage 解析设计 |
| 与 openai provider 抢 `/v1/embeddings` 等路径 | 不会：dashscope 仅匹配 `/v1/services`，`/v1/embeddings` 仍走 openai native 路径表 |
| dashscope 上游不参与模型自动探测（`upstream/catalog.rs` `model_catalog_probe_paths` 无该分支），GUI 模型列表需手填 | 纯透传最小入侵的预期行为；家族说明行与维护手册提示 |

### 5.5 测试计划

| 用例 | 期望 |
| --- | --- |
| 配置 dashscope 上游 + `POST /v1/services/aigc/text-generation/generation` | plan.provider == "dashscope"、无 transform、出站 URL = base+prefix+版本段替换路径、Authorization 为上游 key |
| 配置 dashscope + `POST /v1/services/...`（多 dashscope 上游） | 按 priority/strategy 选上游（既有调度语义） |
| **未配置** dashscope + `POST /v1/services/...` | 落 formatless 兜底，行为与改动前一致（回归用例） |
| 非 `/v1/services` 路径 + dashscope 已配置 | 不受影响：`/v1/chat/completions`、`/v1/messages`、`/v1/embeddings` 路由回归 |
| compose 单测 | `family_compose("dashscope")` 纯拼接、版本段替换、未配置家族恒等回退 |
| 前端 form round-trip | url_compose.dashscope 序列化/反序列化、空家族省略 |
| url-compose-editor 渲染 | 勾选 dashscope 出现家族区、prefix 示例 `/api`、MAP 弹窗示例、placeholder 字号类存在 |
| 实机端到端 | 真实百炼 Key 调 multimodal-generation 与 text-embedding，HTTP 200 + 语义正确 |
| 路径 `/v1/servicesX`（非段边界）+ dashscope 已配置 | 不命中 dashscope，走既有兜底链 |
| CLI `serve` 加载含 `url_compose.dashscope` 配置 | 加载成功，转发与 GUI 一致（共用 config/runtime crate） |

### 5.6 实现拆分

1. **Rust**：PATCH 1（const + dispatch）+ PATCH 2（compose 家族）+ 单测 → `cargo test -p token_proxy_runtime -p token_proxy_config`
2. **前端**：PATCH 3（types/options/editor/i18n）+ Vitest → `pnpm test` / `tsc`
3. **验证**：实机端到端（见 5.5 末行）
4. **CLI**：零改动——`token_proxy_cli serve` 与 GUI 共用 `token_proxy_config`/`token_proxy_runtime`，dashscope 段随 PATCH 2 天然可读；配置链路无 `deny_unknown_fields`（仅 `ModelCapabilities` 有），旧二进制读新配置时新键被 serde 静默忽略

改动内聚，单分支单次交付；不拆 PR。

### 5.7 维护文档（随实现更新）

- 全部注入点携带 `MY-DASHSCOPE-PASSTHROUGH PATCH N (part)` 标记（决策 F），供 pull 后重放
- `AGENTS.md`：若实现涉及其中描述的代理行为边界，同步一句 dashscope 透传说明（按"改了 AGENTS.md 记载的内容就必须更新它"的规则执行）
- 本设计文档状态改为"已实现"并补实现日期；研究笔记 §5 追加"已实现"指向标记名

## 6. 成功标准（可验证）

1. GUI 多选框出现并可选 `dashscope`；勾选后 url_compose 面板出现 dashscope 家族（prefix 示例 `/api`，默认后缀预填）
2. `POST /v1/services/aigc/multimodal-generation/generation` 在配置正确时返回 DashScope 真实响应（实机 200）
3. 拼接窗口空态提示字号明显小于输入字号，无横向溢出
4. 未配置 dashscope 时全部既有路由与 formatless 行为不变（回归测试通过）
5. `cargo test`（runtime/config）、`pnpm test`、`tsc` 全绿
6. CLI `serve` 加载含 `url_compose.dashscope` 段的配置成功，转发行为与 GUI 一致

## 7. Open Questions

无——技术细节已按"与原软件兼容且最小入侵"原则定稿（决策 A-F）。
