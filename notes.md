# Notes: Token Proxy 更新升级实现

## Sources

### Source 1: .github/workflows/release.yml
- Key points:
  - 预发布由 main 分支 push 触发（Test 成功后）。
  - 预发布版本来自 scripts/versioning.mjs prerelease，tag 形如 vX.Y.Z-N。
  - 正式发布通过 workflow_dispatch，tag 形如 vX.Y.Z。
  - 使用 tauri-apps/tauri-action 构建并发布 Release。

### Source 2: scripts/versioning.mjs
- Key points:
  - prerelease 版本：在 next patch 上追加数字（如 0.1.2-1）。
  - prerelease tag：v${version}（例如 v0.1.2-1）。
  - release 版本必须为 x.y.z，tag 为 vX.Y.Z。

### Source 3: src-tauri/tauri.conf.json
- Key points:
  - 当前未启用 updater 插件。
  - 版本号 0.1.1。

### Source 4: Tauri v2 Updater docs
- Key points:
  - updater 签名不可禁用。
  - 构建时需要环境变量 `TAURI_SIGNING_PRIVATE_KEY` 与可选 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
  - `createUpdaterArtifacts: true` 会生成 `latest.json` 与签名文件。

## Synthesized Findings

### Release/Channel 现状
- stable 对应 vX.Y.Z 的 release。
- beta 对应 prerelease：vX.Y.Z-N（非固定 tag）。
- 因此 beta 需要一个“稳定 URL”来指向最新 prerelease 的 latest.json。

### Signing/Env 结论
- Tauri v2 统一使用 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
