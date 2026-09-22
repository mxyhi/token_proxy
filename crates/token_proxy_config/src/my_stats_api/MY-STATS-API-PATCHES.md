# MY-STATS-API-PATCHES —— 上游更新后恢复只读统计 HTTP API

对齐 `my_url_compose/MY-URL-COMPOSE-PATCHES.md` 的恢复惯例。官方 main 不含本模块；
每次 `git pull` 官方后需：检出 `my_stats_api/` 两个目录 → 重新注入 4 处 PATCH → 验证。
（2026-09-22 锚点适配：原 `MY-CONF-OPENAI` 锚点所在方案已由 `my-url-compose` 替代，
本指南锚点全部更新为当前分支真实代码位置。）

## 文件清单（纯新增，零上游冲突）

| 目录 | 内容 |
|---|---|
| `crates/token_proxy_config/src/my_stats_api/` | `mod.rs`（配置段解析 / 端口推导 / 全局同步 / 鉴权判定）+ `mod.test.rs` |
| `crates/token_proxy_app/src/my_stats_api/` | `mod.rs`（axum 守护：`/max_id`、`/rows`、`/attribution` 三端点 + 鉴权中间件） |

## PATCH 1 —— `crates/token_proxy_config/src/lib.rs`

**锚点**：搜 `MY-URL-COMPOSE PATCH C0 END`（其下、`mod normalize;` 之上）。

```rust
// ══════════ MY-STATS-API PATCH 1 (mod) START ══════════
pub mod my_stats_api;
// ══════════ MY-STATS-API PATCH 1 (mod) END ══════════
```

**锚点**：函数 `fn build_runtime_config(config: ProxyConfigFile)` **函数体首行**（注释块之前）。

```rust
fn build_runtime_config(config: ProxyConfigFile) -> Result<ProxyConfig, String> {
    // ══════════ MY-STATS-API PATCH 1 (sync) START ══════════
    let _ = my_stats_api::sync_settings(&config);
    // ══════════ MY-STATS-API PATCH 1 (sync) END ══════════
    let log_level = config.log_level;
```

**替代锚点**：mod 区搜 `mod normalize;`。

## PATCH 2 —— `crates/token_proxy_config/src/types.rs`（⚠ 两处锚点，缺一编译失败）

**锚点 2a（字段）**：`ProxyConfigFile` 结构体内 `pub upstreams: Vec<UpstreamConfig>,`（其下、结构体结束 `}` 之前）。

```rust
// ══════════ MY-STATS-API PATCH 2 (field) START ══════════
/// 本地增强：只读行级统计 HTTP API（my_stats_api 模块）。
#[serde(default, skip_serializing_if = "Option::is_none")]
pub my_stats_api: Option<crate::my_stats_api::MyStatsApiSection>,
// ══════════ MY-STATS-API PATCH 2 (field) END ══════════
```

**锚点 2b（Default impl）**：`impl Default for ProxyConfigFile` 内 `upstreams: Vec::new(),`（其下）。

```rust
// ══════════ MY-STATS-API PATCH 2 (default) START ══════════
my_stats_api: None,
// ══════════ MY-STATS-API PATCH 2 (default) END ══════════
```

**替代锚点**：搜 `max_request_body_bytes` 字段带（字段区）与 `impl Default for ProxyConfigFile`（default 区）。

## PATCH 3 —— `crates/token_proxy_app/src/app.rs`

**锚点**：函数 `TokenProxyApp::open` 末尾，搜 `token proxy app composed`（该 tracing 行之后、`Ok(Self {` 之前）。

```rust
// ══════════ MY-STATS-API PATCH 3 (spawn) START ══════════
crate::my_stats_api::spawn(paths.clone());
// ══════════ MY-STATS-API PATCH 3 (spawn) END ══════════
```

**注意**：spawn 必须用 `std::thread::spawn` + 线程内自建 runtime（模块内已实现），
**不得改成 `tokio::spawn`**——`open` 是同步函数，GUI 的 tauri setup 线程没有 tokio
runtime 上下文，会直接 panic。

## PATCH 4 —— `crates/token_proxy_app/Cargo.toml`

**锚点**：`token_proxy_runtime = { path = "../token_proxy_runtime" }`（其下）：

```toml
# ══════════ MY-STATS-API PATCH 4 (deps) START ══════════
axum = "0.8.9"
sqlx = { version = "0.9", features = ["runtime-tokio", "sqlite"] }
# ══════════ MY-STATS-API PATCH 4 (deps) END ══════════
```

**锚点**：`tokio = { version = "1.53", features = ["rt", "sync", "time"] }` → features 补 `net`（TcpListener 需要）：

```toml
tokio = { version = "1.53", features = ["rt", "sync", "time", "net"] }
```

## config.jsonc 示例

```jsonc
// GUI: %APPDATA%\com.mxyhi.token-proxy\config.jsonc ｜ CLI: C:\token-proxy\config.jsonc
"my_stats_api": {
  "enabled": true,          // 默认 false；不启用则零监听
  "host": "127.0.0.1",      // 允许 "0.0.0.0"（局域网）——此时必须配置 token
  "port": 9308,             // 缺省 = 代理端口 + 100（代理 9208 → 9308）
  "token": "my-secret"      // 缺省 / null / "" = 无鉴权（仅限回环或可信内网）
}
```

- `token` 变更：proxy 运行中保存配置 → reload 即生效；**proxy 停止状态保存 → 下次启动生效**
- `host` / `port` 变更：重启应用生效
- debug 构建默认代理端口是 19208 → 统计端口缺省 19308

## 验收（curl，PowerShell / bash 通用）

```bash
# 水位探测（无鉴权示例）
curl http://127.0.0.1:9308/max_id
# 增量行（Bearer 优先；?token= 会进访问日志，仅兼容）
curl -H "Authorization: Bearer my-secret" "http://127.0.0.1:9308/rows?afterId=0&limit=10"
# 归因复查
curl -H "Authorization: Bearer my-secret" "http://127.0.0.1:9308/attribution?cid=<client_request_id>"
# 错误 token → 401
curl -i -H "Authorization: Bearer wrong" http://127.0.0.1:9308/max_id
```

## Windows 侧恢复 / 构建（PowerShell，构建克隆 `C:\WSL\Ubuntu2204WSL\Dev\git-clone\token-proxy\token_proxy`）

```powershell
git checkout main; git pull origin main
git checkout feat/my-stats-api -- crates/token_proxy_config/src/my_stats_api/ crates/token_proxy_app/src/my_stats_api/
# 按 PATCH 1/2/3/4 重新注入官方文件（grep "MY-STATS-API PATCH" 检查是否残留）
cargo test -p token_proxy_config
pnpm install; pnpm tauri build     # 桌面安装包：src-tauri\target\release\bundle\nsis\*-setup.exe
cargo install --path crates/token_proxy_cli --locked --profile release   # CLI
```
