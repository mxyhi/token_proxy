# Token Proxy 的 Magpie 基座

- `prelease` 从上游 Magpie 提交 `6f91fd0e74bb238dbe2bec7d565cc35f42a74bdf` 开始；旧 Rust/Tauri 代码保留在 `main` 和 `chore/update-magpie-upstream`，后续按功能移植。
- `origin` 为 `https://github.com/mxyhi/token_proxy.git`，`upstream` 为 `https://github.com/yetone/magpie.git`。同步时使用 `git fetch upstream`，在迁移分支审查合并。
- 主要应用标题、平台应用标识、安装包及项目反馈链接使用 Token Proxy。Go 模块路径、内部 `magpie` 命令、协议、插件接口和上游版权保留，避免本轮同时改动所有集成边界。
- 启动入口把配置/缓存命名空间设为 `token-proxy-magpie`。默认配置为 `~/.config/token-proxy-magpie`（支持 XDG 及上游便携模式），不读取或自动迁移旧版 Tauri / 官方 Magpie 配置。旧版所用端口和数据格式也不自动继承。
- 本轮自更新采用手动下载：入口关闭上游更新调度，显式检查更新返回本仓库 Releases 地址。上游签名验证代码不变。未来启用自更新需要为此发行版建立独立的签名、校验和更新源，不能只替换 URL。
- `magpie://`、内部可执行文件名和部分上游文案暂时保留；Windows 包内仍为 `magpie.exe`，供上游 CLI launcher 使用。当前阶段不保证与官方 Magpie 同时运行时的协议注册、默认端口互不冲突。

## 自动预发布

推送 `prelease` 触发 `.github/workflows/prelease.yml`，并行调用上游测试工作流及五组平台构建；全部通过后发布同一提交的 GitHub 预发布。版本为 `0.2.0-prelease.<run_number>`，标签为 `prelease-0.2.0-<run_number>`，不会触发上游 `v*` 发布，也不会替换稳定版 Latest。

产物包括 macOS arm64/amd64 的 DMG 和 ZIP、Linux amd64/arm64 的 tar.gz、Windows amd64/arm64 的 ZIP，以及 SHA256SUMS。macOS 仅 ad-hoc 签名，未经 Apple 开发者签名/公证。所有发布仅使用当前仓库的 GITHUB_TOKEN，不需要上游的 RELEASES_REPO_TOKEN。

## 安装与升级

macOS/Linux 需要 `curl` 和 `jq`（macOS 可用 `brew install jq`，Debian/Ubuntu 可用 `sudo apt install curl jq`）。运行：

```sh
curl -fsSL https://raw.githubusercontent.com/mxyhi/token_proxy/prelease/install.sh | sh
```

脚本从自有仓库最近 100 条 Release 中选择发布时间最新的 `prelease-*` 非草稿预发布，核对资产存在且上传完成，再下载对应架构的包和 `SHA256SUMS`。下载或校验失败保留现有安装；重新运行即可升级。可通过 `TOKEN_PROXY_TAG=prelease-0.2.0-4` 指定版本（变量放在管道右侧 `sh` 前）。这是手动调用安装器，应用内自动更新仍关闭。

- macOS：安装 `Token Proxy.app` 到 `/Applications`，不可写时使用 `~/Applications`；同名旧 Tauri 应用会被拒绝覆盖，可用 `TOKEN_PROXY_APP_DIR` 指定其他目录。替换失败恢复原应用。
- Linux：安装 `token-proxy` 到 `~/.local/bin`，需要 glibc、GTK 3 和 WebKitGTK 4.1，Debian/Ubuntu 可安装 `libwebkit2gtk-4.1-0`。当前没有独立 nogui 或 Android/Termux 预发布包；脚本不自动安装系统依赖或注册桌面菜单。
- Unix 命令为 `token-proxy`，可用 `TOKEN_PROXY_BIN_DIR` 指定绝对路径；不创建或覆盖官方 `magpie` 命令。旧版 CLI 与本预发布同名时请用独立目录。README 的 Unix 命令示例采用此名称；Windows 包内仍为 `magpie.exe`。
- Windows：从 [Token Proxy Releases](https://github.com/mxyhi/token_proxy/releases) 下载对应 ZIP。

脚本只提供安装/升级，不提供上游的 `--gui`、`--tui`、`--uninstall` 等参数。网络代理使用 curl 的 `HTTPS_PROXY`/`ALL_PROXY` 环境变量。校验和来自相同 GitHub Release，不代表 Apple 公证或独立的发布签名。

仓库 `site/` 保留上游站点源码，未作为 Token Proxy 官网发布；其中官方安装器、Docker/Nix/Termux 下载与自动更新说明不适用于本发行版。模块路径、插件源、`magpie://` 协议和上游在线服务未在本次安装入口修正中重命名。


验证：`node --test build/token-proxy-install.test.mjs`、`sh -n install.sh`、`go test ./internal/appdir ./internal/update`、`sh -n build/token-proxy-release.sh`、`actionlint`；完整多平台测试由 CI 执行。实际构建可运行 `sh build/token-proxy-release.sh darwin-arm64 0.2.0-prelease.1`。
