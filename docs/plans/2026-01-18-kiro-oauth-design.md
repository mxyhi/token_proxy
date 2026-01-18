# Kiro OAuth 登录与上游绑定设计

## 目标
- 在 Token Proxy 内复刻 CLIProxyAPIPlus 的 Kiro OAuth 能力（AWS Builder ID 设备码/授权码流、Google/GitHub OAuth）。
- 支持多账户，并与 upstreams 列表融合：provider=kiro 的上游必须绑定一个账户。
- 高性能与简洁：异步 I/O，最少状态，清晰错误提示。

## 非目标
- 不在 config.jsonc 内存储 OAuth token。
- 不实现额外兼容层或历史格式迁移（遵循 No backward compatibility）。

## 数据与存储
- 账户文件目录：`AppConfig/kiro-auth/`（与 config.jsonc 同一基目录）。
- 文件命名：`kiro-{provider}-{idPart}.json`（provider=aws|google|github|idc）。
- 账户字段：`access_token`、`refresh_token`、`expires_at`、`profile_arn`、`auth_method`、`provider`、`client_id`、`client_secret`、`email`、`last_refresh`、`start_url`、`region`。
- 上游绑定：新增 `upstreams[].kiro_account_id`（provider=kiro 时必填）。

## 后端架构
- 新模块：`src-tauri/src/kiro/`
  - `oauth_flows`: Builder ID 设备码/授权码流、Google/GitHub OAuth。
  - `token_store`: 账户读写、索引、列表、删除。
  - `token_refresh`: 基于 refresh_token 的刷新与重试。
  - `management_api`: Tauri command 接口。
- 关键常量对齐 CLIProxyAPIPlus：
  - OIDC 端点 `https://oidc.us-east-1.amazonaws.com`
  - Builder ID start URL `https://view.awsapps.com/start`
  - Kiro OAuth 基址 `https://prod.us-east-1.auth.desktop.kiro.dev/login`
  - 回调端口/路径 `9876` + `/kiro/callback`
  - 临时回调文件 `.oauth-kiro-{state}.oauth`

## 请求处理与刷新
- provider=kiro 请求时，从 `kiro_account_id` 解析账户并注入 `Authorization: Bearer <access_token>`。
- 401/403 自动刷新：刷新成功后更新文件并重试一次；失败则标记过期并要求重新登录。
- 若未绑定或找不到账号，直接返回 401（带清晰错误信息）。

## 前端 UI（upstreams 融合）
- Upstreams 表格新增“账户”列（仅在 provider=kiro 行展示）。
- Upstream Editor 添加 Kiro 账号选择器：
  - 未登录：选择登录方式（AWS Builder ID / Google / GitHub）。
  - 登录中：显示设备码或等待回调状态。
  - 已登录：显示账号摘要与过期时间，支持切换/登出。
- 校验：provider=kiro 时必须选择账号。

## API / Tauri Commands
- `kiro_list_accounts`：返回账号列表与状态。
- `kiro_start_login(method, upstreamId?)`：生成 OAuth URL/设备码并启动登录。
- `kiro_poll_login(state)`：设备码轮询或回调状态。
- `kiro_logout(accountId)`：删除账号文件并解绑。
- `kiro_attach_account(upstreamId, accountId)`：绑定账号到上游。

## 测试与验证
- Rust 单测：token_store 读写/索引，refresh 成功/失败分支。
- 手动验证：设备码登录、社交登录、绑定后请求成功、401/403 自动刷新。
- 前端流程：登录/切换/登出状态流转与校验提示。

## 风险与处理
- AWS OIDC 的授权码流在不同 SDK 中支持度存在差异：严格复用 CLIProxyAPIPlus 的实现路径，必要时回退设备码流。
