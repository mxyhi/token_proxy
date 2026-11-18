# Codex 模型目录

`codex-models.json` 是 TP 维护的公共目录，同时作为编译时离线回退。

运行时来源：https://raw.githubusercontent.com/mxyhi/token_proxy/main/crates/token_proxy_runtime/resources/codex-models.json

当前官方元数据来自 OpenAI Codex 提交 `6c0cee502d215e5df2dd9a303cf2e252229c9e12`（2026-11-17，新增 `gpt-6.2-astra`、`gpt-6.2-sol`，`gpt-6.1-sol` 上下文更新为 500,000）：
https://github.com/openai/codex/blob/6c0cee502d215e5df2dd9a303cf2e252229c9e12/codex-rs/models-manager/models.json
原始文件 SHA-256：`d50b6749506d97e5f334dbabcb666eb1689f423cf3e88cfabdcf3c536b873068`。许可证见 CODEX-LICENSE。

TP 仅调整数组顺序和 `priority`（版本递减、同版本价格由高到低），移除 `gpt-5.5`；其余元数据和未知字段保留。隐藏内部模型不出现在普通菜单。

更新时从固定官方提交取文件，应用上述菜单规则，使用目标 Codex CLI 的 `model/list` 验证后提交此 JSON 到 main；无需修改版本或发布安装包。首次访问及缓存到期时同步刷新，单次限时 3 秒；成功缓存 15 分钟，失败 1 分钟后重试。并发请求使用上次有效目录。下载上限 900 KiB，预留 Codex 1 MiB 响应限制中的本地额外模型空间。失败不清空内存缓存；进程重启且离线时使用内置快照。

此文件不含本地额外模型与凭据；GET /codex/models.json 按每次请求的授权范围追加额外模型。目录格式须兼容已发布客户端；若 Codex 引入破坏性 schema，需先升级 TP 校验与兼容逻辑，不能只更新 JSON。
