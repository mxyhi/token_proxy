# 用独立预发布分支渐进迁移到 Magpie

Token Proxy 的 `prelease` 采用 Magpie 的 Go/Wails 源码作为新基座，旧 Rust/Tauri 主线继续保留。发行身份与配置目录独立，内部模块和协议沿用上游，以降低后续同步及逐项功能移植成本；这不是旧数据格式的原地升级。迁移期间从自有 Releases 手动升级，待独立签名链建立后再启用自更新。
