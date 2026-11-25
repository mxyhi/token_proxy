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

验证：`go test ./internal/appdir ./internal/update`、`sh -n build/token-proxy-release.sh`、`actionlint`；完整多平台测试由 CI 执行。实际构建可运行 `sh build/token-proxy-release.sh darwin-arm64 0.2.0-prelease.1`。
