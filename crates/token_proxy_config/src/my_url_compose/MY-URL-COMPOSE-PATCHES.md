# MY-URL-COMPOSE PATCHES —— 上游 pull 后恢复指南

本模块（`crates/token_proxy_config/src/my_url_compose/`）为 fork 本地增强，上游无此代码。
官方代码只保留下列横幅标记注入点。`git pull` 上游后，若某注入点冲突/丢失，按本文
锚点上下文重新注入。完整维护流程见
`../docs/项目维护/260922-01-gui和api请求逻辑优化/维护手册-main对齐指南.md`。

| PATCH | 文件 | 内容 |
|---|---|---|
| C0 | `crates/token_proxy_config/src/lib.rs` | `pub mod my_url_compose;` 模块声明 |
| C1 | `crates/token_proxy_config/src/types.rs` | `UpstreamConfig` 新增 `url_compose` 可选字段 |
| C2 | `crates/token_proxy_config/src/types.rs` + `normalize.rs` | `UpstreamRuntime` 新增字段 + 构造点填充 + 旧 base 迁移告警 |
| C3 | `crates/token_proxy_config/src/types.rs` + `runtime/proxy/upstream/prepare.rs` | `upstream_url` 签名加 `provider` 参数、函数体替换为纯拼接；调用方传参 |
| R1 | `crates/token_proxy_runtime/src/proxy/upstream/catalog.rs` + `my_probe_gate.rs` | 模型探测仅账户型上游 |
| G5 | `src/features/update/UpdateNotifier.tsx` | `runAutoCheck` 自动检查短路 |

## C0 lib.rs 模块声明

```rust
mod model_mapping;
// ══════════ MY-URL-COMPOSE PATCH C0 START ══════════
pub mod my_url_compose;
// ══════════ MY-URL-COMPOSE PATCH C0 END ══════════
mod normalize;
```

## C1 types.rs UpstreamConfig 字段（`overrides` 字段之后追加）

锚点：`pub struct UpstreamConfig` 内末尾字段为
`pub overrides: Option<UpstreamOverrides>,`

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<UpstreamOverrides>,
    // ══════════ MY-URL-COMPOSE PATCH C1 START ══════════
    /// 渠道级出站地址组合（prefix + suffix，第一段为版本段）；缺省恒等回退。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_compose: Option<my_url_compose::UrlComposeConfig>,
    // ══════════ MY-URL-COMPOSE PATCH C1 END ══════════
}
```

## C2 types.rs UpstreamRuntime 字段 + normalize.rs 构造点

`pub struct UpstreamRuntime` 末尾字段 `pub allowed_inbound_formats: InboundApiFormatMask,` 之后：

```rust
    // ══════════ MY-URL-COMPOSE PATCH C2 START ══════════
    /// 出站地址组合（加载期已规范化；缺省全 None = 恒等回退）。
    pub url_compose: my_url_compose::UrlComposeConfig,
    // ══════════ MY-URL-COMPOSE PATCH C2 END ══════════
```

`normalize.rs` `normalize_single_upstream` 中 `let header_overrides = ...` 之后：

```rust
    // ══════════ MY-URL-COMPOSE PATCH C2 START ══════════
    let url_compose = upstream
        .url_compose
        .as_ref()
        .map(|value| value.normalized())
        .unwrap_or_default();
    crate::my_url_compose::warn_legacy_versioned_base_url(
        &upstream.id,
        &upstream.base_url,
        upstream.url_compose.as_ref(),
    );
    // ══════════ MY-URL-COMPOSE PATCH C2 END ══════════
```

构造体 `UpstreamRuntime { ... }` 内追加 `url_compose: url_compose.clone(),`
（放在 `allowed_inbound_formats,` 之后）。

## C3 types.rs upstream_url + prepare.rs 调用方

`upstream_url` 函数（原官方实现含 `strip_overlapping_prefix` 调用）整体替换：

```rust
    // ══════════ MY-URL-COMPOSE PATCH C3 START ══════════
    /// 出站 URL：纯拼接 + 版本段替换（见 my_url_compose 模块文档）。
    pub fn upstream_url(&self, provider: &str, path_with_query: &str) -> String {
        my_url_compose::compose_outbound_url(self, provider, path_with_query)
    }
    // ══════════ MY-URL-COMPOSE PATCH C3 END ══════════
```

`runtime/proxy/upstream/prepare.rs` 唯一调用方改为
`let upstream_url = upstream.upstream_url(provider, &upstream_path_with_query);`
（`provider` 为该函数既有参数）。

同时删除官方函数 `strip_overlapping_prefix`、`normalize_openai_compatible_path_for_base_url`、
`is_bigmodel_coding_plan_base_url`（含各自文档注释）。

## R1 catalog.rs 探测门控

`collect_model_discovery_jobs` 收集循环内，对每条 `upstream` 先判
`crate::proxy::upstream::my_probe_gate::should_probe(upstream)`，false 则 `continue`。
（`my_probe_gate.rs` 为独立新文件，非注入。）

## G5 UpdateNotifier.tsx 自动检查短路

`runAutoCheck` 函数体首行：

```ts
      // ═══ MY-URL-COMPOSE PATCH G5 START：关闭自动更新检测 ═══
      if (reason !== "manual") {
        return;
      }
      // ═══ MY-URL-COMPOSE PATCH G5 END ═══
```

> 实施备注：`reason` 参数恒为自动侧传入的描述串，manual 走独立入口；以
> `checkForUpdate({ source: "manual" })` 调用点不经过本函数为准。
