# MY-PROXY-ENDPOINT-PATCHES —— 上游更新后恢复仪表盘「代理接入地址」栏

对齐 `my_stats_api/MY-STATS-API-PATCHES.md` 与 `my_url_compose/MY-URL-COMPOSE-PATCHES.md` 的恢复惯例。
官方 main 不含本模块；每次 `git pull` 官方后需：检出 3 个自有目录 → 重新注入 5 组 PATCH（6 处物理位置，含 2 处前端）→ 重放 i18n key 块 → 验证。

功能 = **3 个自有目录 + 5 组横幅注入（4 个上游文件、约 8 行）+ 1 个 i18n key 块**。

## 文件清单（纯新增，零上游冲突）

| 目录 | 内容 |
|---|---|
| `crates/token_proxy_config/src/my_proxy_endpoint/` | `mod.rs`（段结构 / 容错反序列化 / 扫描门控 / 快照归一化 / 回落 / 选择校验）+ `mod.test.rs` + 本文件 |
| `src-tauri/src/my_proxy_endpoint.rs` | 网卡扫描（`sysinfo`）、读配置、按需落盘、2 个 Tauri 命令 + 单测 |
| `src/features/my-proxy-endpoint/` | `types.ts` / `endpoint.ts`（纯函数）/ `api.ts` / `MyProxyEndpointBar.tsx` / 两个测试 |

## 依赖

零新增依赖。`src-tauri/Cargo.toml` 早已声明 `sysinfo = "0.39"`（此前全仓无调用点），本功能首次使用它；
`Cargo.lock` 无需改动。若上游将来移除 `sysinfo`，需重新加入该依赖行。

## PATCH 1 —— `crates/token_proxy_config/src/lib.rs`

**锚点**：搜 `MY-STATS-API PATCH 1 (mod) END`（其下、`mod normalize;` 之上）。

```rust
// ══════════ MY-PROXY-ENDPOINT PATCH 1 (mod) START ══════════
pub mod my_proxy_endpoint;
// ══════════ MY-PROXY-ENDPOINT PATCH 1 (mod) END ══════════
```

**替代锚点**：mod 声明区搜 `mod normalize;`；再不行就找 `pub mod my_url_compose;`。

## PATCH 2 —— `crates/token_proxy_config/src/types.rs`（⚠ 两处锚点，缺一编译失败）

**锚点 2a（字段）**：`ProxyConfigFile` 结构体内 `MY-STATS-API PATCH 2 (field) END` 之下、结构体结束 `}` 之前；
替代锚点 `pub upstreams: Vec<UpstreamConfig>,`。

```rust
    // ══════════ MY-PROXY-ENDPOINT PATCH 2 (field) START ══════════
    /// 本地增强：仪表盘「代理接入地址」栏的持久化段（my_proxy_endpoint 模块）。
    #[serde(
        default,
        deserialize_with = "crate::my_proxy_endpoint::de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub my_proxy_endpoint: Option<crate::my_proxy_endpoint::MyProxyEndpointSection>,
    // ══════════ MY-PROXY-ENDPOINT PATCH 2 (field) END ══════════
```

**锚点 2b（Default impl）**：`impl Default for ProxyConfigFile` 内 `MY-STATS-API PATCH 2 (default) END` 之下；
替代锚点 `upstreams: Vec::new(),`。

```rust
            // ══════════ MY-PROXY-ENDPOINT PATCH 2 (default) START ══════════
            my_proxy_endpoint: None,
            // ══════════ MY-PROXY-ENDPOINT PATCH 2 (default) END ══════════
```

> 只补 `Default` 即可：仓库内所有 `ProxyConfigFile` 字面量构造都带 `..Default::default()` 或直接用 `::default()`
> （`account_upstreams.rs`、`proxy/service/tests.rs`、`my_stats_api/mod.test.rs`）。

## PATCH 3 —— `src-tauri/src/lib.rs`（mod 声明）

**锚点**：mod 声明区 `mod logging;` 与 `mod tray;` 之间。

```rust
mod logging;
// ══════════ MY-PROXY-ENDPOINT PATCH 3 (mod) START ══════════
mod my_proxy_endpoint;
// ══════════ MY-PROXY-ENDPOINT PATCH 3 (mod) END ══════════
mod tray;
```

**替代锚点**：`mod commands;` 附近任意 mod 声明行。

## PATCH 4 —— `src-tauri/src/lib.rs`（`generate_handler!`）

**锚点**：`.invoke_handler(tauri::generate_handler![` 内 `preview_client_setup,` 之下。

```rust
            preview_client_setup,
            // ══════════ MY-PROXY-ENDPOINT PATCH 4 (handler) START ══════════
            my_proxy_endpoint::my_proxy_endpoint_snapshot,
            my_proxy_endpoint::my_proxy_endpoint_select,
            // ══════════ MY-PROXY-ENDPOINT PATCH 4 (handler) END ══════════
            write_claude_code_settings,
```

## PATCH G0 / G1 —— `src/features/dashboard/DashboardPanel.tsx`（前端挂载）

**锚点 G0（import）**：`import { m } from "@/paraglide/messages.js"` 之后。

```tsx
// ══════════ MY-PROXY-ENDPOINT PATCH G0 (import) START ══════════
import { MyProxyEndpointBar } from "@/features/my-proxy-endpoint/MyProxyEndpointBar"
// ══════════ MY-PROXY-ENDPOINT PATCH G0 (import) END ══════════
```

**锚点 G1（挂载）**：`return (` 后第一层 `<div className="flex flex-col gap-4">` 的首个子节点。

```tsx
  return (
    <div className="flex flex-col gap-4">
      {/* ══════════ MY-PROXY-ENDPOINT PATCH G1 (mount) START ══════════ */}
      <MyProxyEndpointBar />
      {/* ══════════ MY-PROXY-ENDPOINT PATCH G1 (mount) END ══════════ */}
      {status === "error" ? (
```

**替代锚点**：若上游重构 `DashboardPanel`，改为挂到 `src/features/dashboard/pages/dashboard-page.tsx` 的
`<DashboardPanel />` 同级（`<AppShell>` 内、`<DashboardPanel />` 之前），组件自带的 `px-4 lg:px-6` 已对齐既有内边距。

## PATCH i18n —— `messages/zh.json` + `messages/en.json`

在文件末尾追加 `my_proxy_endpoint_*` key 块（当前实现共 25 个，中英一一对应，数量必须相等）：

`my_proxy_endpoint_title / _loading / _unavailable / _listen_label / _ip_label / _format_label /`
`_ip_kind_loopback / _ip_kind_lan / _ip_kind_virtual /`
`_format_openai / _format_anthropic / _format_gemini / _no_suffix /`
`_copy_url / _copy_ip / _copy_key / _copied / _copied_ip / _copied_key / _copy_failed /`
`_key_label / _key_empty / _hint_scan / _hint_loopback / _update_failed`

追加后必须跑 `pnpm i18n:compile`；缺失 key 会让组件在运行时报 `is not a function`。

## config.jsonc 示例

```jsonc
// GUI: %APPDATA%\com.mxyhi.token-proxy\config.jsonc ｜ CLI: C:\token-proxy\config.jsonc
"my_proxy_endpoint": {
  "current_ip": "192.168.1.23",     // 上次选中的 IP
  "current_format": "openai",        // openai | anthropic | gemini
  "available_ips": [                 // 启动扫描快照；host 非 0.0.0.0 时省略
    "192.168.1.23",
    "172.20.0.3",
    "10.0.0.5",
    "127.0.0.1"
  ]
}
```

行为要点（勿当 bug）：

- host 为 `0.0.0.0` / `::` 才扫描；`127.0.0.1` / `localhost` / 空串不扫描，界面固定显示该 host 与配置端口且不可编辑。
- `current_ip` 失效时回落顺序：首个 `192.*` → 首个 `172.*` → `127.0.0.1`（**故意不选 `10.*`**，按需求定义）。
- `available_ips` 永远包含 `127.0.0.1`；段缺失且从未扫描时，配置文件不会出现该键（零变化）。
- 切换 IP / 格式立即写盘，只改本段；**不触发代理 reload**，也不进 `save_proxy_config` 编排。

## 验收

```bash
cargo test -p token_proxy_config      # 含 my_proxy_endpoint 12 例
cargo test -p token_proxy my_proxy_endpoint   # 扫描纯函数 3 例
pnpm i18n:compile && pnpm vitest run  # 含 endpoint 9 例 + 组件 7 例
pnpm build
```

手工：host=`0.0.0.0` 首次打开仪表盘 → 配置文件出现 `available_ips`；反复展开下拉不再变；
host=`127.0.0.1` → 下拉禁用、显示 `127.0.0.1:<port>`；配置页保存一次后该段仍在（extras 透传）。

## Windows 侧恢复 / 构建（PowerShell，构建克隆 `C:\WSL\Ubuntu2204WSL\Dev\git-clone\token-proxy\token_proxy`）

见 `../docs/项目维护/260923-03-proxy端接入地址/维护手册-main对齐指南.md` 第 7 步。
