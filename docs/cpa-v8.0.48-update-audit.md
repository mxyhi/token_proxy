# CPA v8.0.41–v8.0.48 适用更新接入审计

核对日期：2026-10-31。范围固定为 [router-for-me/CLIProxyAPI v8.0.40...v8.0.48](https://github.com/router-for-me/CLIProxyAPI/compare/v8.0.40...v8.0.48)。

基线 `d946c4fba6fc9a5cfbca1f3d85184d62ad2fb683`，终点 `dbb706215a3afd2f0ea3c96482d4233856466caa`。GitHub compare 分页与本地 git rev-list 均为 **128 个唯一提交（109 个非 merge、19 个 merge）**；完整树差异414文件。未使用受限的GitHub文件首屏代替完整差异。

## 交付范围

实现适用于现有协议、用量、凭据和HTTP转发的更新，保留前一轮Azure原生工具namespace与单条加密历史恢复修复。本次纳入 v0.1.226 发布；下文记录发布前源码与独立CLI验证，最终发布状态及各平台资产以 GitHub Release 为准。

## 证据分组

### P：协议、工具与历史

- `runtime/src/proxy/anthropic_compat/{request,response}.rs`、`response/{responses_to_anthropic,anthropic_to_responses}.rs`、`claude_reasoning.rs`：commentary转thinking并用明确内部标记回放；不把真实reasoning签名当commentary。原生Claude发送前恢复为文本；thinking token明细保留显式0。
- `runtime/src/proxy/gemini_compat/{request,response,stream,partial_args}.rs`：无content终态、安全阻断、malformed、工具索引、工具失败语义；Vertex partialArgs按路径有界组装，完整调用后才发参数。每candidate一个活动调用、最多16个candidate，1MiB/64层/16384节点/最大数组索引4095；不承诺逐字参数流。
- `protocol/src/gemini_tools/{refs,schema}.rs`：保留$ref同级约束与allOf合取，避免闭合对象被放宽；延后array占位items。
- `protocol/src/{tool_identity,xai_client_tools}.rs`、`runtime/src/proxy/openai_compat/{mod,input}.rs`：内联tool_addition、根工具优先与namespace子工具合并，错误标记跨中间格式保留，原生tsc_/ctc_历史ID不误删。
- 当前Gemini签名按opaque字段保存，没有CPA Vertex/Antigravity签名识别矩阵；不声称具备独立目标隔离。Claude5.5转换入口保留现有forced tool_choice明确拒绝；原生请求由供应商验证，未静默改成auto。后者是有意保留调用语义，不称为CPA自动降级的等价实现。
- 回归：`response/cpa_protocol.test.rs`、`openai_compat/cpa_v8.test.rs`、Gemini stream/response测试、Anthropic tests、schema.test.rs。

### U：用量与观察

- `storage/src/usage/{mod,observations}.rs`：稀疏快照按字段合并，空/metadata帧不清零，更新分量后不沿用旧total；失败流保存已收到用量。记录完成工具/图片数和供应商缓存/媒体元数据；output_item.done遇到空终态仍保留，Gemini分片调用去重。
- `protocol/src/openai_usage.rs`：Chat/Responses双向保留完整token明细和显式0；reasoning不重复加到输出。
- `runtime/src/proxy/{log,image_request_details}.rs`与`upstream/request_body.rs`：从最终发送的JSON/multipart提取图片参数白名单；使用multer 3.1解析边界，跳过文件、提示词、URL和base64。参数观察失败不影响请求转发。
- 新观察保存在`usage_json._proxy_observed`，与供应商原始usage分开。保持原token定价，不推断工具/图片费用，不做历史账单回填，不改数据库schema。
- `log.rs::ClientRequestBilling`已有client_request_id/attempt_index；Rust上下文拥有所有权，无Gin复用问题。已有标准SSE分片、多行及50MiB行/事件预算；不照搬Go性能优化或无对应入口的Interactions。
- 回归：storage usage tests、OpenAI usage tests、图片参数与失败请求日志测试。

### R：运行时、鉴权与安全

- `response/dispatch/{buffered,stream}.rs`、`upstream/result.rs`：明确capacity code优先，quota type/code不会被capacity文案覆盖；HTTP和首输出前SSE保持一致。临时容量不标坏账户。
- `upstream_selector/mod.rs`与`upstream/prepare.rs`：按provider/实际凭据selector_key/映射后的模型记住Retry-After，下一次请求仍跳过该模型，其他模型/凭据可用；支持秒数和HTTP-date。当前请求不立即原地重试，最终429保留等待提示。
- `service/mod.rs`与`account_selector/mod.rs`：重登已有app lifecycle会应用配置并重建状态，从而清除普通短失败；强制等待共享到新状态，热加载前的在途响应稍后返回等待提示也有效。等待仅保存在本进程，应用重启不承诺持久化。
- `account_codex/{error,store}.rs`：永久拒绝的refresh凭据不重复发送，仍有效access保留；新refresh恢复、旧结果不污染新凭据，诊断不回显OAuth正文。
- `response/streaming.rs`、`codex_compat/stream.rs`及`protocol/src/sse.rs`：过滤私有metadata/subscription_usage，保留原生Codex允许的metadata；只有event字段声明私有类型时也可识别，不把无换行的损坏片段边界猜成标准SSE。
- `http_client.rs`：普通HTTP只自动跟随同源重定向，跨源直接返回3xx；xAI继续禁全部重定向。原来依赖跨域跳转的渠道需直接配置最终地址。
- `server/inbound.rs`：未认证请求立即401并关闭HTTP/1连接，详情日志只留请求头，不等待上传完正文。
- 已有输出后截断不重试/不回写账户冷却、取消归499、CORS Authorization。回归包括真实本地重定向目标零命中、pending请求体即时401、跨请求模型等待、重载等待继承与账户72项测试。

### M：模型目录

- Gemini原生`/v1beta/models`/详情走FormatTransform::None，保留上游token limits；没有CPA Home合成Gemini目录而丢失limits的路径（`server/tests.rs::gemini_models_index_route_dispatches_to_gemini`）。
- 本地`upstream/catalog.rs::build_model_catalog_response_body`保留upstream_id/model前缀，并有prefix模式回归；不采用CPA按provider重注册、覆盖prefix的注册器。
- `model_discovery.rs`、`upstream/catalog.rs::refresh_model_discovery`和Dashboard/Codex模型菜单已有按需刷新，失败保留最后成功快照。
- `codex_catalog_cache.rs::parse_catalog`与`server/codex_catalog.rs`使用JSON Value保留上游新增限制字段；内置目录已有GPT-6.1条目。不用CPA静态目录覆盖本项目额外模型和定制目录。
- `model_capabilities.rs`按请求和配置判断原生搜索，模型目录读取快照；无CPA全局registry写锁队列，不引入其索引架构。

### S：本地存储与管理差异

- 请求日志经`storage/src/log.rs`写SQLite，不用客户端request-id拼日志文件名。
- 账户由`account_codex/store.rs`及account_store paths管理；没有CPA auth-dir相对路径的cooldown sidecar、Git仓库packfile或文件watcher绑定问题。
- `account_lifecycle/mod.rs`已有登录提交、绑定应用及回滚；`upstream/prepare.rs`已有上游proxy_url与应用OAuth代理回退。CPA管理API新增“每OAuth会话代理”是独立管理产品，不在本次数据面兼容范围。
- Rust JSON Map直接增删，没有Go sjson每次删除都复制整个文档的对应实现。

### C：压缩差异

`upstream/request.rs::enforce_identity_accept_encoding`强制identity，override不能开启压缩；当前reqwest未启用gzip/brotli/zstd，无下游压缩池、zstd请求日志解压或WebSocket压缩链。不新增未经测量的压缩配置、encoder池和相关Go benchmark。相关header合同回归继续覆盖。

### X：无对应产品的变更

`config/src/normalize.rs::normalize_providers`支持openai/openai-response/anthropic/gemini/kiro/codex/xai；`runtime/Cargo.toml`与`server/bootstrap.rs`未提供CPA插件ABI、Home集群、Redis RESP、mDNS广播、Responses WebSocket/duplex、Interactions、Devin、Kimi及AI Studio relay。Gemini兼容输入可处理partialArgs不等于新增Vertex OAuth产品。

xAI图片仍按原请求转发，不存在CPA payload规则引擎的默认值覆盖问题。`cpa-ag-compact-v1:`胶囊是CPA Antigravity特有产物，当前系统不生成也不解封；原生encrypted/compaction维持opaque，不借此删除未知历史。上游README推广、Go测试、Go lint与相应合并不单独移植。

以上源码路径省略共同前缀`crates/token_proxy_`：例如`runtime/src/...`对应`crates/token_proxy_runtime/src/...`。

## 验证

- `cargo test --workspace`：**1370项通过，0失败、0忽略，无编译警告**。其中runtime859、protocol106、storage108、Codex账户72；应用、配置、其他账户及Tauri测试均通过。
- `cargo build -p token_proxy_cli --offline`、`cargo fmt --all -- --check`、`git diff --check`通过。
- 最终构建独立CLI真实Azure `gpt-6-astra`：namespace工具调用与结果回传两次HTTP200，最终`OK`；单条合成无效reasoning经过受限恢复后HTTP200/`OK`。
- 真测只在权限700临时目录保存权限600配置副本；用户当前禁用的Azure渠道只在副本启用。启动独立端口、独立SQLite，结束后停止进程并删除副本；真实配置哈希前后相同。
- 测试过程中首次探测遇到502；随后夹具缺namespace description得到400，补齐必填项后供应商成功返回。ax默认把正文截至2000字符，改用输出文件读取完整JSON才完成往返断言。这些探测不作为模型/额度或长期稳定性结论。
- 前轮同源码模型目录直连与本地代理均取到474条；本轮未重测目录，目录不等于部署清单。未完整重放用户旧会话；未对其他供应商发真实收费请求。
- 以上验证针对本地源码与独立CLI；未替换正在运行的桌面应用。各平台正式打包及签名由发布工作流完成。

## 完整提交覆盖

每行链接固定commit；组号指向上文源码和回归证据。合并项与原提交一并列出，不重复计为独立功能。

| 提交 | 上游变更 | 本地判定 | 证据组 |
|---|---|---|---|
| [e45f9426](https://github.com/router-for-me/CLIProxyAPI/commit/e45f942645ca646b5eeb7af58a599a1e5d2c9cb3) | feat(home): add cluster latency telemetry and probing | 不适用：无Home集群/订阅器 | X |
| [41a4b9f4](https://github.com/router-for-me/CLIProxyAPI/commit/41a4b9f4deb68a0be6535e57407b05eaf7d39a25) | Merge pull request #6903 from router-for-me/home | 合并项：行为见同主题非merge提交及最终树 | X |
| [376015c3](https://github.com/router-for-me/CLIProxyAPI/commit/376015c35feac763f08cd37cafb6c65a727b7e3d) | fix(codex): map commentary phase messages to thinking blocks in Claude response translator | 已接入适用行为 | P |
| [e1fb2521](https://github.com/router-for-me/CLIProxyAPI/commit/e1fb25219c09cdcb6b6f90d3ec612ddf670d15fc) | fix(claude): preserve tool arguments for implicitly open empty object schemas | 已接入适用行为 | P |
| [e5434f65](https://github.com/router-for-me/CLIProxyAPI/commit/e5434f65c851a1483c13a4f717c49423152fcfbb) | fix(signature): sign the first Gemini function call after a text part | 已有适用行为；不引入专用识别器 | P |
| [83cb1b95](https://github.com/router-for-me/CLIProxyAPI/commit/83cb1b95eb47f97a62ba01f3c07fc0bb71fb06e1) | fix(gemini): populate token limits in Gemini model catalog responses | 已有对应目录/只读机制 | M |
| [3f5d3a16](https://github.com/router-for-me/CLIProxyAPI/commit/3f5d3a161b0861f18c6fa1d101e9a0446448ad33) | fix(gemini/vertex): preserve non-content finish reasons and surface prompt blocks | 已接入适用行为 | P |
| [29150a22](https://github.com/router-for-me/CLIProxyAPI/commit/29150a2262b4e9d102986c7775d794c7122670a7) | fix(schema): preserve Gemini $ref sibling constraints | 已接入适用行为 | P |
| [888f2382](https://github.com/router-for-me/CLIProxyAPI/commit/888f23824d163dc1c4e569db9a96981701945437) | fix(api): add RESP protocol bounds validation and connection panic recovery | 不适用：无Redis RESP/Git存储/Home集群 | X |
| [8289ec4c](https://github.com/router-for-me/CLIProxyAPI/commit/8289ec4c71ad914d0612c2776352b76ba3d355a6) | fix(usage): preserve interactions token semantics when merging stream usage | 已接入现有协议的用量行为 | U |
| [0d3ca1d3](https://github.com/router-for-me/CLIProxyAPI/commit/0d3ca1d3b7cf94feef80fb44bfa4b125c5165751) | fix(store,home): handle repository index and packfile corruption and wrap Redis connections with cancelable dialer | 不适用：无Redis RESP/Git存储/Home集群 | X |
| [ab139b45](https://github.com/router-for-me/CLIProxyAPI/commit/ab139b45057b30a1a84ff628a5af8694fc7407a2) | fix(kimi): share thinking replay across K3.1 Pro variants | 不适用：无Kimi/Vertex OAuth/Devin专用执行器 | X |
| [da21f1e5](https://github.com/router-for-me/CLIProxyAPI/commit/da21f1e58ed5551426bd41f12db9d655e146415a) | fix(antigravity): restore non-stream tool signatures and preserve stream call indices | 已接入适用行为 | P |
| [9d08f0d4](https://github.com/router-for-me/CLIProxyAPI/commit/9d08f0d430ed1e7942d621e4b8559aa3879fa849) | fix(pluginhost): strip credential headers across cross-origin redirects | 已接入 | R |
| [44d1ddf5](https://github.com/router-for-me/CLIProxyAPI/commit/44d1ddf53887bd276e3493f84f100a17a2716df0) | fix(pluginhost): translate plugin count_tokens replies as token counts | 不适用：无CPA插件ABI/宿主 | X |
| [35e834da](https://github.com/router-for-me/CLIProxyAPI/commit/35e834da2466f89f909cfeed34bb38c499863595) | feat(pluginabi): carry retry-after metadata from plugin executor errors | 不适用：无CPA插件ABI/宿主 | X |
| [50147aec](https://github.com/router-for-me/CLIProxyAPI/commit/50147aec62b672e12ffd6ee2d457ebc3079570c5) | fix(home): close underlying transport synchronously when canceling rebalance RPCs | 不适用：无Home集群/订阅器 | X |
| [e42eee6e](https://github.com/router-for-me/CLIProxyAPI/commit/e42eee6e1ce7f98c1fc211ac266404de9989eaa4) | fix(pluginhost): apply user payload rules to plugin executor requests | 不适用：无CPA插件ABI/宿主 | X |
| [c9af5290](https://github.com/router-for-me/CLIProxyAPI/commit/c9af5290a6d28cc305fa1c92d971e236e6727277) | fix(pluginhost): run canonical thinking pipeline before plugin execution | 不适用：无CPA插件ABI/宿主 | X |
| [179d43dc](https://github.com/router-for-me/CLIProxyAPI/commit/179d43dc744695b719bddb3e3e4be0239af04a73) | fix(logging): prevent path traversal vulnerabilities in request log filenames | 不适用CPA文件存储；本地使用SQLite与应用数据路径 | S |
| [bc064340](https://github.com/router-for-me/CLIProxyAPI/commit/bc0643405a1d0adf77dd0f109972ac3313d66397) | docs: remove outdated third-party extensions and apps from readmes | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [2bb6ac6f](https://github.com/router-for-me/CLIProxyAPI/commit/2bb6ac6faa2ab24db501212e4ccf2c3c9db446ed) | fix(lint): resolve go vet lostcancel and static analysis warnings from v8.0.42 | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [6340fe43](https://github.com/router-for-me/CLIProxyAPI/commit/6340fe4312a9bf9cb0c383c30c4d4e33c5f3c1f8) | fix(codex): bind duplex websocket turns to requested client model after interceptor rewrites | 不适用：无Responses WebSocket/duplex端点 | X |
| [3e7a21e2](https://github.com/router-for-me/CLIProxyAPI/commit/3e7a21e2f3cc0c6fcf991c150255194e33aeeb7e) | fix(openai): isolate duplex turn reauthorization from gin context and enforce bound principal | 不适用：无Responses WebSocket/duplex端点 | X |
| [32dd2c86](https://github.com/router-for-me/CLIProxyAPI/commit/32dd2c86b557b4a02dbe8b19baeab39b4ab0f8b6) | fix(watcher): preserve file snapshot binding on persisted auth updates | 不适用CPA文件存储；本地使用SQLite与应用数据路径 | S |
| [21553509](https://github.com/router-for-me/CLIProxyAPI/commit/21553509662eba4b8e889d797822c1891c980b5d) | fix(auth): restore re-logged credentials to rotation after refresh failures | 已接入 | R |
| [b2fda98e](https://github.com/router-for-me/CLIProxyAPI/commit/b2fda98ea831143887c7c356d01be3d9055d8880) | fix(util): defer Gemini array placeholder items until after ref and allOf resolution | 已接入适用行为 | P |
| [6b56ce6c](https://github.com/router-for-me/CLIProxyAPI/commit/6b56ce6c28b82b21283570fb13d2176f8006606f) | perf(thinking): skip document copies when deleting absent fields | 不移植Go JSON复制优化；Rust直接操作Map，未发现待修性能缺口 | S |
| [e48d780f](https://github.com/router-for-me/CLIProxyAPI/commit/e48d780f9b251241d133f70539bd4a54f64f4fa9) | fix(plugin): preserve caller scope for replayed Claude OAuth MCP tool aliases | 不适用：无CPA插件ABI/宿主 | X |
| [0c61b12c](https://github.com/router-for-me/CLIProxyAPI/commit/0c61b12cf2262e8369a2bfa72e81f0edfcd85fc4) | fix(xai): deduplicate tools when promoting historical additional_tools | 已接入适用行为 | P |
| [ce433b37](https://github.com/router-for-me/CLIProxyAPI/commit/ce433b3727aa356650372636c9a48aedafb26f30) | fix(cliproxy): back off home subscriber retries | 不适用：无Home集群/订阅器 | X |
| [86d951cd](https://github.com/router-for-me/CLIProxyAPI/commit/86d951cd5090a644fe6de4f0a1f79ff67f484f91) | fix(openai/responses): drop response.metadata for non-Codex clients | 已接入 | R |
| [cf4fc4e7](https://github.com/router-for-me/CLIProxyAPI/commit/cf4fc4e720ff2c74b4ad8af0d212990076c0d180) | Merge pull request #6969 from router-for-me/home | 合并项：行为见同主题非merge提交及最终树 | X |
| [ba06ec58](https://github.com/router-for-me/CLIProxyAPI/commit/ba06ec58a109d668aafc838f8ea77971025f7b37) | fix(vertex): default credential location to global | 不适用：无Kimi/Vertex OAuth/Devin专用执行器 | X |
| [bdde391d](https://github.com/router-for-me/CLIProxyAPI/commit/bdde391d53b66092d53d596a857a893a462dfeb0) | fix(translator): preserve finishReason in Codex and OpenAI to Gemini streams | 已有适用行为；不引入专用识别器 | P |
| [3dae075d](https://github.com/router-for-me/CLIProxyAPI/commit/3dae075de2724d0d24140a170774b855ca5b4253) | feat(devin): aggregate swe-2 lightning UIDs into swe-2-lightning model | 不适用：无Kimi/Vertex OAuth/Devin专用执行器 | X |
| [27f3abfb](https://github.com/router-for-me/CLIProxyAPI/commit/27f3abfb70a6d05d4f11b49e5d0bc70c05e1ef14) | feat(management): support session-scoped OAuth proxy with per-account persistence (#6973) | 不引入CPA管理端OAuth会话代理产品；沿用现有应用/上游代理合同 | S |
| [0fe4cc2c](https://github.com/router-for-me/CLIProxyAPI/commit/0fe4cc2cfa804fe254f1efe958da432ac1bb8af7) | fix(openai): report buffered stream error when responses websocket data channel closes first | 不适用：无Responses WebSocket/duplex端点 | X |
| [90fe5b23](https://github.com/router-for-me/CLIProxyAPI/commit/90fe5b23d3509be4ef48aa44938ce26a8f5f4d9c) | fix(handlers): report stream cancellation when channel closes with canceled context | 已有且回归验证 | R |
| [84e83441](https://github.com/router-for-me/CLIProxyAPI/commit/84e83441472c83d96937ca0d9eeb4f7e7344c781) | test(executor): fix race conditions in flaky websocket executor tests | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [30780351](https://github.com/router-for-me/CLIProxyAPI/commit/307803510bce87f986fcf7b1a84190f105c7b4aa) | fix(aistudio): translate every SSE frame of a relay stream chunk | 不适用AI Studio relay；共用标准SSE已有分片重组 | X |
| [712e4970](https://github.com/router-for-me/CLIProxyAPI/commit/712e4970bc9811f36e6b0a9d87a62d54a4bbc586) | fix(handlers): report cancellation when stream closes before first chunk | 已有且回归验证 | R |
| [e5321f53](https://github.com/router-for-me/CLIProxyAPI/commit/e5321f535cafa17ca8775b3500938432af5bf27a) | fix(auth,pluginapi,usage): share per-selection ID between Scheduler.Pick and UsageRecord | 已有请求/尝试归属与拥有所有权的上下文 | U |
| [8365de86](https://github.com/router-for-me/CLIProxyAPI/commit/8365de86b6b1bac5f25f66d6b1c6fc742e04c7da) | docs(readme): update Kimi model availability | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [064e008b](https://github.com/router-for-me/CLIProxyAPI/commit/064e008b079fae13e58c7d4007322482eb600695) | feat(codex): add full-chain compression for Codex upstream and downstream traffic | 不适用：不协商压缩，无对应编解码器 | C |
| [bc807104](https://github.com/router-for-me/CLIProxyAPI/commit/bc80710469dd28a5f96b415f15473646799cc736) | perf: index web-search support to remove registry write-lock convoy | 已有对应目录/只读机制 | M |
| [43f5386b](https://github.com/router-for-me/CLIProxyAPI/commit/43f5386bc3b904caa6106c879dd5dbc2d1e86ceb) | fix(translator): map Claude thinking_tokens to reasoning_tokens in OpenAI chat usage | 已接入适用行为 | P |
| [531721bf](https://github.com/router-for-me/CLIProxyAPI/commit/531721bfcee1982712c645a649048dc02e927f4e) | fix(codex): restore Codex commentary history via internal thinking signature marker | 已接入适用行为 | P |
| [058b803b](https://github.com/router-for-me/CLIProxyAPI/commit/058b803b2cce45ff5fde7905dada3086aba7567d) | fix(api): explicitly allow Authorization in CORS preflight responses | 已有且回归验证 | R |
| [6fe654bf](https://github.com/router-for-me/CLIProxyAPI/commit/6fe654bfc6b62309b9cda2367832cef9feb20ce2) | perf(pluginhost): gate request interceptor body delivery behind schema v7 opt-out | 不适用：无CPA插件ABI/宿主 | X |
| [ede07caf](https://github.com/router-for-me/CLIProxyAPI/commit/ede07cafa64a5a485590e30f9470bdc6b2ebc850) | fix(interactions): emit [DONE] sentinel after terminal usage frame in Antigravity and Gemini streams | 不适用：无Interactions协议入口 | X |
| [8a982a52](https://github.com/router-for-me/CLIProxyAPI/commit/8a982a5210197a0b98837297cde73a1ad1f7e4c6) | feat(discovery): align TXT schema with draft-su-ai-gateway-discovery-00 | 不适用：无mDNS发现/管理广播 | X |
| [b214385c](https://github.com/router-for-me/CLIProxyAPI/commit/b214385c1cf31b5aee6885b108538663731a2610) | chore(config): deprecate discovery advertise-management | 不适用：无mDNS发现/管理广播 | X |
| [1d45a5f3](https://github.com/router-for-me/CLIProxyAPI/commit/1d45a5f3329025414ee430341daf2686cf0c029a) | fix(claude): handle forced tool restrictions in claude 5.5 | 保留明确拒绝合同，不静默降级 forced tool_choice | P |
| [37bd18b0](https://github.com/router-for-me/CLIProxyAPI/commit/37bd18b08d9c6d6116412b128c9712b29af14dc4) | fix(antigravity): accept bare content-block lists in interactions input | 不适用：无Interactions协议入口 | X |
| [11e5f67b](https://github.com/router-for-me/CLIProxyAPI/commit/11e5f67bdb36c42af94ab5338440975f2b4f3746) | fix(discovery): resolve split mDNS responses with scoped bounded caches | 不适用：无mDNS发现/管理广播 | X |
| [999526c7](https://github.com/router-for-me/CLIProxyAPI/commit/999526c76def7063e9baa09c1ea2750e8cf5a5b6) | docs(discovery): describe LAN resolution limits and trust boundaries | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [aa4130e6](https://github.com/router-for-me/CLIProxyAPI/commit/aa4130e64c84546aa3f0c5db23af3961b62ba14e) | fix(gemini): keep truncated HTTP 200 streams out of credential cooldown and failover | 已有且回归验证 | R |
| [ec81e70a](https://github.com/router-for-me/CLIProxyAPI/commit/ec81e70aa2238da2d6455c39fbd8aeab175d5148) | Merge pull request #7010 from sususu98/feat/discovery-ietf-draft | 合并项：行为见同主题非merge提交及最终树 | X |
| [58a4d04f](https://github.com/router-for-me/CLIProxyAPI/commit/58a4d04f32919f3f5c07e34a477c5abc6542b66a) | fix(signature): recognize Vertex bare-Tink Gemini thought signatures | 已有适用行为；不引入专用识别器 | P |
| [1398f273](https://github.com/router-for-me/CLIProxyAPI/commit/1398f273905b4281d8ae37601b38bd01293642e8) | fix(signature): scope Vertex bare-Tink replay to Vertex targets | 已有适用行为；不引入专用识别器 | P |
| [1f917e77](https://github.com/router-for-me/CLIProxyAPI/commit/1f917e77c4562623aa49e9804d999524151a2f96) | docs(readme): remove APIMart sponsor sections and assets | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [c3767123](https://github.com/router-for-me/CLIProxyAPI/commit/c37671233fd2f8bdba1f3c47107d43997bc6ad08) | Merge pull request #6959 from JohnsonRan/patch/thinking-delete-missing-nocopy | 合并项：行为见同主题非merge提交及最终树 | S |
| [3c948a58](https://github.com/router-for-me/CLIProxyAPI/commit/3c948a5808971ffe2b150414f36ec2fab0b671be) | perf(executor): decode upstream zstd synchronously and sniff unlabeled bodies lazily | 不适用：不协商压缩，无对应编解码器 | C |
| [b4cbda59](https://github.com/router-for-me/CLIProxyAPI/commit/b4cbda5915101b1f913e048847209d01037d90d1) | docs(readme): add GPT-6.1 Terra | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [9a35fa12](https://github.com/router-for-me/CLIProxyAPI/commit/9a35fa12b2f388b235afec48736b7dfecfcc4de5) | fix(codex): treat turn_rate_limit_exceeded as transient capacity with retry hint | 已接入 | R |
| [19398539](https://github.com/router-for-me/CLIProxyAPI/commit/19398539ff69ddb0dc004d858417b8e5d42d3fc9) | feat(codex): add workspace and cloud agent orchestrator restrictions to client models | 已有对应目录/只读机制 | M |
| [a8582fba](https://github.com/router-for-me/CLIProxyAPI/commit/a8582fbaca90d5c17d0e3a93d8e3259d7b919f58) | fix(signature): pin Vertex bare-Tink recognition to observed key ids | 已有适用行为；不引入专用识别器 | P |
| [023dd2ad](https://github.com/router-for-me/CLIProxyAPI/commit/023dd2adca74de73896f8aa25eb789b35f188de3) | Merge pull request #7016 from sususu98/fix/vertex-gemini-thought-signature-7007 | 合并项：行为见同主题非merge提交及最终树 | P |
| [be929f07](https://github.com/router-for-me/CLIProxyAPI/commit/be929f07fa3c294371f3ead12d22c3c56f7d821e) | fix(interactions): normalize gemini stream sse lines | 不适用：无Interactions协议入口 | X |
| [7517549a](https://github.com/router-for-me/CLIProxyAPI/commit/7517549a64879b1dec6adf90bb023361d64c21e7) | fix(signature): do not reject Antigravity CAQS signatures by channel ID | 已有适用行为；不引入专用识别器 | P |
| [437386db](https://github.com/router-for-me/CLIProxyAPI/commit/437386db57b888c9bce7a72d0da7907dede0f1a7) | Merge pull request #7042 from sususu98/fix/antigravity-caqs-channel-id | 合并项：行为见同主题非merge提交及最终树 | P |
| [908e95d9](https://github.com/router-for-me/CLIProxyAPI/commit/908e95d94b617330ae3d78778bb2e6403447087a) | fix(models): preserve channel prefix in Gemini model names during prefix registration | 已有对应目录/只读机制 | M |
| [10417e5b](https://github.com/router-for-me/CLIProxyAPI/commit/10417e5b6b85461e8d7072a6fea6ad9f781b1f0f) | feat(registry,management): support on-demand v8 management refresh for model catalogs | 已有对应目录/只读机制 | M |
| [f73e66ac](https://github.com/router-for-me/CLIProxyAPI/commit/f73e66ac2a0d783c829e7c4fc2683206376a5fd4) | fix(responses-ws): keep downstream websocket open on upstream Codex disconnect | 不适用：无Responses WebSocket/duplex端点 | X |
| [b4f59d70](https://github.com/router-for-me/CLIProxyAPI/commit/b4f59d7035da9b0ff014d8555ab71e76f590241a) | fix(devin): log apply_patch rejection diagnostics and recover complete bare patches | 不适用：无Kimi/Vertex OAuth/Devin专用执行器 | X |
| [826bb167](https://github.com/router-for-me/CLIProxyAPI/commit/826bb167802f147c40c1f2eba6c36b6aafbf8601) | fix(images): apply payload default rules to xAI image endpoints | 不适用：没有payload default/default-raw/override规则引擎 | X |
| [64316127](https://github.com/router-for-me/CLIProxyAPI/commit/643161274449d6bf92977060af2a756265d5b4cf) | feat(translator): support Vertex streaming functionCall partialArgs conversion | 已接入适用行为 | P |
| [86a1aabf](https://github.com/router-for-me/CLIProxyAPI/commit/86a1aabf0178be8b188cfa2c9d2fedc5fcc48a02) | fix(codex): retry another credential on early websocket 429 without closing downstream | 不适用：无Responses WebSocket/duplex端点 | X |
| [c6425a6e](https://github.com/router-for-me/CLIProxyAPI/commit/c6425a6e6c7aaa9b113cfc3caa1c30d5b2b0f765) | docs: add Ebb to "Who is with us?" | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [b7442218](https://github.com/router-for-me/CLIProxyAPI/commit/b74422180e867ac24bf5da4820acac40099eb974) | fix(auth,usage): snapshot request state to avoid reading reused gin.Context (#7056) | 已有请求/尝试归属与拥有所有权的上下文 | U |
| [477605dc](https://github.com/router-for-me/CLIProxyAPI/commit/477605dc6deb301e587dff646f9f43ea8b58ee60) | fix(api): prevent incomplete unauthenticated request bodies from exhausting descriptors | 已接入 | R |
| [ad8bdd3f](https://github.com/router-for-me/CLIProxyAPI/commit/ad8bdd3f20bef44f0684b8a6096a216b2c76040d) | fix(middleware): bound zstd decompression output and window in request logging | 不适用：不协商压缩，无对应编解码器 | C |
| [a9e0d48a](https://github.com/router-for-me/CLIProxyAPI/commit/a9e0d48a66245b0873133f983ee3bc03419c2d9c) | Merge pull request #7057 from lucasnsaraujo/docs/add-ebb | 合并项：行为见同主题非merge提交及最终树 | X |
| [f6d29ec7](https://github.com/router-for-me/CLIProxyAPI/commit/f6d29ec7330d58a765e1e282336d46fae639bace) | Merge pull request #6989 from kdcokenny/docs/kimi-k31-availability | 合并项：行为见同主题非merge提交及最终树 | X |
| [ba26dea0](https://github.com/router-for-me/CLIProxyAPI/commit/ba26dea03f211e41a21a31cb5f799a09fc5ab53c) | Merge pull request #7023 from JohnsonRan/patch/response-decode-sync-zstd | 合并项：行为见同主题非merge提交及最终树 | C |
| [65889d39](https://github.com/router-for-me/CLIProxyAPI/commit/65889d39f0bdc6f60e31c863ae993579cc6d9e1e) | test: update tests for default template changes and environment compatibility | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [16c6b4bd](https://github.com/router-for-me/CLIProxyAPI/commit/16c6b4bd8c033b770ebd432d1b39fe53e61d1ee2) | perf(api): reuse response compression encoders with a smaller zstd window | 不适用：不协商压缩，无对应编解码器 | C |
| [12f2cafe](https://github.com/router-for-me/CLIProxyAPI/commit/12f2cafe33b31b7ba245462c168240da7db5b50e) | test(api): measure open-stream heap with empty encoder pools | 不适用：不协商压缩，无对应编解码器 | C |
| [e30575dc](https://github.com/router-for-me/CLIProxyAPI/commit/e30575dc77748c0702db5285764703a20e9e45a7) | Merge pull request #6995: index web-search support | 合并项：行为见同主题非merge提交及最终树 | M |
| [58874f3b](https://github.com/router-for-me/CLIProxyAPI/commit/58874f3bf3923c75339a820fa9dd5bda4dacd23b) | feat(registry): index web-search support | 已有对应目录/只读机制 | M |
| [f6ee1e64](https://github.com/router-for-me/CLIProxyAPI/commit/f6ee1e64d83363aa9b933a57305ce4dc5bd26f87) | Merge pull request #7022: reuse response compression encoders | 合并项：行为见同主题非merge提交及最终树 | C |
| [a3e76891](https://github.com/router-for-me/CLIProxyAPI/commit/a3e76891b1a149d9cb4dd718dd54db06f976d987) | refactor(api): reuse response compression encoders and add tests | 不适用：不协商压缩，无对应编解码器 | C |
| [f7342c38](https://github.com/router-for-me/CLIProxyAPI/commit/f7342c3838c98511ac506be7f99f09bf708eb214) | Merge pull request #6970 from soulmachine/responses-drop-metadata-non-codex | 合并项：行为见同主题非merge提交及最终树 | R |
| [ae65e26d](https://github.com/router-for-me/CLIProxyAPI/commit/ae65e26df2f8074bd79c47251ffc032947fb75a5) | feat(api): filter private metadata in OpenAI responses and improve SSE framing recovery | 已接入 | R |
| [9717e325](https://github.com/router-for-me/CLIProxyAPI/commit/9717e325ba6ab2c7483ea93ccb5f2047eb2e7dd5) | fix(responses): reject late websocket interrupts after terminal events | 不适用：无Responses WebSocket/duplex端点 | X |
| [e324e481](https://github.com/router-for-me/CLIProxyAPI/commit/e324e4814c2fa45cbc00608a240896fe354c6f59) | perf(pluginhost): allow frontend auth providers to opt out of request body delivery | 不适用：无CPA插件ABI/宿主 | X |
| [0fe38224](https://github.com/router-for-me/CLIProxyAPI/commit/0fe38224e5292855b1a3afdcf036c0a185b082d7) | feat(pluginhost): support raw request body byte-range patching | 不适用：无CPA插件ABI/宿主 | X |
| [535110e1](https://github.com/router-for-me/CLIProxyAPI/commit/535110e109e49ca3fd8811f47aae4244c79b5714) | docs(readme): add Nia Live Demo to "Who is with us?" | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [3f2b8f28](https://github.com/router-for-me/CLIProxyAPI/commit/3f2b8f2815e3362af18b7af7d683c8116e26b0fa) | Merge pull request #7082 from nia-teams-admin/docs/add-nia-live-demo | 合并项：行为见同主题非merge提交及最终树 | X |
| [90c447d3](https://github.com/router-for-me/CLIProxyAPI/commit/90c447d3a8727d115aa7552ec93ce023736ed492) | fix(responses): use tsc_ item id prefix for Gemini tool search calls | 已接入适用行为 | P |
| [fa42f390](https://github.com/router-for-me/CLIProxyAPI/commit/fa42f390c1207cc08c9251a5c6fdcca2e4140193) | test(openai): assert websocket disconnect error event | 上游文档/Go专用测试与静态检查，无独立Rust改动 | X |
| [02ee3139](https://github.com/router-for-me/CLIProxyAPI/commit/02ee313962ba0fca3b0c1203813dbba92005c3de) | fix(devin): classify JSON control character errors | 不适用：无Kimi/Vertex OAuth/Devin专用执行器 | X |
| [56efc81f](https://github.com/router-for-me/CLIProxyAPI/commit/56efc81fa5246277cda95ca6f9d190719ed166e3) | Merge pull request #7092 from sususu98/fix/slow-test-disconnect | 合并项：行为见同主题非merge提交及最终树 | X |
| [7df437c8](https://github.com/router-for-me/CLIProxyAPI/commit/7df437c8229ed528c214c78a97c39121947a0e3d) | fix(aistudio): buffer fragmented relay SSE frames | 不适用AI Studio relay；共用标准SSE已有分片重组 | X |
| [498170a4](https://github.com/router-for-me/CLIProxyAPI/commit/498170a49cc92558d5ba0130aeb2ca7f25e3b90d) | fix(aistudio): add relay SSE frame reassembly helper | 不适用AI Studio relay；共用标准SSE已有分片重组 | X |
| [89e472f3](https://github.com/router-for-me/CLIProxyAPI/commit/89e472f3251e8600576323dc3f97539c79949630) | feat(usage): capture structured Anthropic Messages upstream usage details | 已接入现有协议的用量行为 | U |
| [24840fdd](https://github.com/router-for-me/CLIProxyAPI/commit/24840fddf8432bf171d68356217e17b5848a1159) | feat(usage): capture structured xAI usage across chat, responses, and media | 已接入现有协议的用量行为 | U |
| [01551f5f](https://github.com/router-for-me/CLIProxyAPI/commit/01551f5f6b260030b35a0fa14f53dcb14a32f3c8) | feat(usage): capture structured OpenAI and Codex usage across responses, chat, and images | 已接入现有协议的用量行为 | U |
| [bd5ec531](https://github.com/router-for-me/CLIProxyAPI/commit/bd5ec5310aba84fcf9150caf513bb4aea842882e) | feat(usage): capture structured Gemini, Vertex, AI Studio, and Antigravity usage | 已接入现有协议的用量行为 | U |
| [d5eb4ca1](https://github.com/router-for-me/CLIProxyAPI/commit/d5eb4ca1d5181f3498bf724035b51a485f924faa) | perf(usage): optimize stream usage observation and large payload parsing | 已有有界流解析；新增观察不保留正文，不移植Go性能代码 | U |
| [97dd3a4a](https://github.com/router-for-me/CLIProxyAPI/commit/97dd3a4a2f0c0c017afe80e3358c431147e2e677) | fix(usage): preserve stream failure usage, multipart image params, and provisional zero search counters | 已接入现有协议的用量行为 | U |
| [a6b4477b](https://github.com/router-for-me/CLIProxyAPI/commit/a6b4477b4f05a780b5b0739eba22302e14c53573) | fix(home): restrict websocket access in home mode | 不适用：无Home集群/订阅器 | X |
| [82deffb9](https://github.com/router-for-me/CLIProxyAPI/commit/82deffb9ca0d5c6b81f47d68e9b56ead09d725f6) | fix(usage): preserve output_item.done tool counts and multiline SSE usage | 已接入现有协议的用量行为 | U |
| [eabcb1b7](https://github.com/router-for-me/CLIProxyAPI/commit/eabcb1b76fa8f1f3dbcc13e7a687340490dd44f3) | Merge pull request #7074 from sususu98/feat/structured-usage-7054 | 合并项：行为见同主题非merge提交及最终树 | U |
| [265ca179](https://github.com/router-for-me/CLIProxyAPI/commit/265ca179839a7de602120e1f51a78a3d28d4aeba) | Merge pull request #7095: reassemble AI Studio SSE frames | 合并项：行为见同主题非merge提交及最终树 | X |
| [d30edd7f](https://github.com/router-for-me/CLIProxyAPI/commit/d30edd7f1e1df97965a5801d4bde0b2ea0b2006a) | Merge pull request #7102 from router-for-me/aistudio | 合并项：行为见同主题非merge提交及最终树 | X |
| [4cb98e3c](https://github.com/router-for-me/CLIProxyAPI/commit/4cb98e3ccdc3852fb27f705358699a39a57d7a0a) | fix(logging): prevent deferred request retention on Responses WebSockets | 不适用：无Responses WebSocket/duplex端点 | X |
| [8dfb45d2](https://github.com/router-for-me/CLIProxyAPI/commit/8dfb45d2c64c01d2018bcb948ad87d273f9c43f5) | Merge pull request #7106 from router-for-me/fix/responses-ws-deferred-logging | 合并项：行为见同主题非merge提交及最终树 | X |
| [43068c86](https://github.com/router-for-me/CLIProxyAPI/commit/43068c8642f3a0ab1ba66056d298006d0829d657) | fix(auth): write the cooldown state next to the auth file when auth-dir is relative | 不适用CPA文件存储；本地使用SQLite与应用数据路径 | S |
| [f013eb66](https://github.com/router-for-me/CLIProxyAPI/commit/f013eb662aa27a29f8163ff29c9e3475b359c264) | fix(openai): keep incomplete streams without [DONE] out of credential cooldown | 已有且回归验证 | R |
| [514d09f7](https://github.com/router-for-me/CLIProxyAPI/commit/514d09f746ff95dc4063440a3c31ff9277a122f7) | feat(codex): guide recovery when automatic credential renewal becomes impossible | 已接入 | R |
| [12d97cfe](https://github.com/router-for-me/CLIProxyAPI/commit/12d97cfe196be266b45f4f7e0cc394c9fdc2e099) | fix(translator): translate mid-conversation Claude tool_addition blocks | 已接入适用行为 | P |
| [67beb7a6](https://github.com/router-for-me/CLIProxyAPI/commit/67beb7a6c097f773b95735b0b200515c2cc2d88e) | Merge pull request #7109 from eaxeax/fix/cooldown-state-relative-auth-dir | 合并项：行为见同主题非merge提交及最终树 | S |
| [bc81923b](https://github.com/router-for-me/CLIProxyAPI/commit/bc81923ba63a99fe35baba18a27724a87f586f9e) | feat(pluginhost): allow model registrars to publish models for multiple providers | 不适用：无CPA插件ABI/宿主 | X |
| [81de4b9c](https://github.com/router-for-me/CLIProxyAPI/commit/81de4b9c48e8d823fa04d68cda09f23059f2964a) | fix(gemini): preserve Claude tool-result error semantics (#6838) | 已接入适用行为 | P |
| [068fe153](https://github.com/router-for-me/CLIProxyAPI/commit/068fe153efa6bdacfcaf68cc840c4ceff372bd72) | fix(antigravity): preserve Claude tool-result error semantics | 已接入适用行为 | P |
| [dbb70621](https://github.com/router-for-me/CLIProxyAPI/commit/dbb706215a3afd2f0ea3c96482d4233856466caa) | fix(codex): expand recognized CPA compaction capsules before forwarding to OpenAI | 不适用：不生成CPA Antigravity专用compaction胶囊 | X |
