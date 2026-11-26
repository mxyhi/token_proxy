# Token Proxy 的 Magpie 基座

- `main` 是 Token Proxy 的迁移主线，从上游 Magpie 提交 `6f91fd0e74bb238dbe2bec7d565cc35f42a74bdf` 开始；旧 Rust/Tauri 代码保留在 `chore/update-magpie-upstream`，后续按功能移植。
- `origin` 为 `https://github.com/mxyhi/token_proxy.git`，`upstream` 为 `https://github.com/yetone/magpie.git`。同步时使用 `git fetch upstream`，在迁移分支审查合并。
- 主要应用标题、平台应用标识、安装包及项目反馈链接使用 Token Proxy。Go 模块路径、内部 `magpie` 命令、协议、插件接口和上游版权保留，避免本轮同时改动所有集成边界。
- 启动入口把配置/缓存命名空间设为 `token-proxy-magpie`。默认配置为 `~/.config/token-proxy-magpie`（支持 XDG 及上游便携模式），不读取或自动迁移旧版 Tauri / 官方 Magpie 配置。发行默认网关端口为 9208，用户保存的端口和 MAGPIE_ADDR 仍优先；旧格式通过显式迁移命令转换。
- 本轮自更新采用手动下载：入口关闭上游更新调度，显式检查更新返回本仓库 Releases 地址。上游签名验证代码不变。未来启用自更新需要为此发行版建立独立的签名、校验和更新源，不能只替换 URL。
- `magpie://`、内部可执行文件名和部分上游文案暂时保留；Windows 包内仍为 `magpie.exe`，供上游 CLI launcher 使用。当前阶段不保证与官方 Magpie 同时运行时的协议注册互不冲突。

## 自动预发布

推送 `main` 触发 `.github/workflows/prelease.yml`，并行调用上游测试工作流及五组平台构建；全部通过后发布同一提交的 GitHub 预发布。版本为 `0.2.0-prelease.<run_number>`，标签为 `prelease-0.2.0-<run_number>`，不会触发上游 `v*` 发布，也不会替换稳定版 Latest。

产物包括 macOS arm64/amd64 的 DMG 和 ZIP、Linux amd64/arm64 的 tar.gz、Windows amd64/arm64 的 ZIP，以及 SHA256SUMS。macOS 仅 ad-hoc 签名，未经 Apple 开发者签名/公证。所有发布仅使用当前仓库的 GITHUB_TOKEN，不需要上游的 RELEASES_REPO_TOKEN。

## 安装与升级

macOS/Linux 需要 `curl` 和 `jq`（macOS 可用 `brew install jq`，Debian/Ubuntu 可用 `sudo apt install curl jq`）。运行：

```sh
curl -fsSL https://raw.githubusercontent.com/mxyhi/token_proxy/main/install.sh | sh
```

脚本从自有仓库最近 100 条 Release 中选择发布时间最新的 `prelease-*` 非草稿预发布，核对资产存在且上传完成，再下载对应架构的包和 `SHA256SUMS`。下载或校验失败保留现有安装；重新运行即可升级。可通过 `TOKEN_PROXY_TAG=prelease-0.2.0-4` 指定版本（变量放在管道右侧 `sh` 前）。这是手动调用安装器，应用内自动更新仍关闭。

- macOS：安装 `Token Proxy.app` 到 `/Applications`，不可写时使用 `~/Applications`；同名旧 Tauri 应用会被拒绝覆盖，可用 `TOKEN_PROXY_APP_DIR` 指定其他目录。替换失败恢复原应用。
- Linux：安装 `token-proxy` 到 `~/.local/bin`，需要 glibc、GTK 3 和 WebKitGTK 4.1，Debian/Ubuntu 可安装 `libwebkit2gtk-4.1-0`。当前没有独立 nogui 或 Android/Termux 预发布包；脚本不自动安装系统依赖或注册桌面菜单。
- Unix 命令为 `token-proxy`，可用 `TOKEN_PROXY_BIN_DIR` 指定绝对路径；不创建或覆盖官方 `magpie` 命令。旧版 CLI 与本预发布同名时请用独立目录。README 的 Unix 命令示例采用此名称；Windows 包内仍为 `magpie.exe`。
- Windows：从 [Token Proxy Releases](https://github.com/mxyhi/token_proxy/releases) 下载对应 ZIP。

脚本只提供安装/升级，不提供上游的 `--gui`、`--tui`、`--uninstall` 等参数。网络代理使用 curl 的 `HTTPS_PROXY`/`ALL_PROXY` 环境变量。校验和来自相同 GitHub Release，不代表 Apple 公证或独立的发布签名。

仓库 `site/` 保留上游站点源码，未作为 Token Proxy 官网发布；其中官方安装器、Docker/Nix/Termux 下载与自动更新说明不适用于本发行版。模块路径、插件源、`magpie://` 协议和上游在线服务未在本次安装入口修正中重命名。


验证：`node --test build/token-proxy-install.test.mjs`、`sh -n install.sh`、`go test ./internal/appdir ./internal/update`、`sh -n build/token-proxy-release.sh`、`actionlint`；完整多平台测试由 CI 执行。实际构建可运行 `sh build/token-proxy-release.sh darwin-arm64 0.2.0-prelease.1`。

## 迁移旧版 Tauri

先安装新版到独立目录，保持旧版可回退。运行以下命令只检查，不修改源目录或目标目录：

```sh
token-proxy migrate-tauri --source "$HOME/Library/Application Support/com.mxyhi.token-proxy"
```

确认检查结果后，关闭新版进程，添加 `--output "$HOME/.config/token-proxy-magpie" --apply`，写入一个**尚不存在**的新配置目录。不能合并到已运行的新实例。迁移程序离线读取 JSONC 和只读 SQLite，不调用旧 OAuth 登录文件，也不刷新 token。正式切换前应备份源目录和 SQLite（包括 WAL 或使用 SQLite backup），先在备用端口验证，再停旧版释放 9208。

保留：监听端口、LAN 开关、全局和上游代理、API keys、禁用状态、本地密钥及作用域、上游模型映射、一个 Codex OAuth 账户。每个密钥的裸模型名映射到独立路由组，成员按旧版 priority 降序排列，使用 order/serial 回退。内部上游 ID 转小写，原大小写引用按新基座的不区分大小写规则继续解析；Codex 上游映射到内置 codex，通过密钥别名保留旧的完整模型引用。本地请求也必须携带有效密钥，错误、禁用、删除的密钥被拒绝（公共 `/` 健康探测除外）。

本阶段要求旧策略为 fill_first + serial，启用的上游有明确模型列表；不支持的账户类型、启用的 passthrough、多个 Codex 账户上游或无本地密钥配置会拒绝迁移，避免静默改变路由。禁用的 passthrough 只保留禁用占位。转换后的路由是迁移时快照，后续新增模型和修改优先级需编辑路由组与密钥别名。

未等价移植的项目会显示在迁移报告中：旧 convert_from_map 白名单、请求超时、请求体上限、CORS、热模型展示、会话冷却及历史用量。原 JSONC 保留为 `tauri-config.original.jsonc`（0600），历史数据库单独归档；不能把归档说成新版统计已导入。先验证新版的鉴权、模型列表和实际请求，再删除旧应用。

迁移后的 `localKeyID` 指向启用的本地密钥（优先 auto 作用域），用于新版连接 Agent 和本机工具请求。禁用或删除它后，这些入口不自动换用另一把权限不同的密钥；现有客户端的原密钥保持不变。

旧版 `available_models` 同时是请求白名单：迁移为每个密钥的明确模型权限，不能放宽为上游通配符。Codex 请求限制到旧配置绑定的账户，避免新基座自动发现的其他账户参与回退。

上游代理空值迁移为 direct，`$app_proxy_url` 展开为旧全局代理地址。导入的 Codex 账户标记 `tauriImported`，即使 Codex 客户端没有订阅登录也由网关独立提供，不修改客户端 auth.json。

旧数据库中的 invalid Codex 账户保留失效标记，后台读取和请求不重用该凭据；账户仍可见，重新登录才能恢复。
