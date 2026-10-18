# Codex 官方模型目录

`codex-models-0.163.0.json` 原样来自 OpenAI Codex `rust-v0.163.0`：
https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/models-manager/models.json

许可证见 CODEX-LICENSE。保留完整官方元数据和未知字段。更新时取对应版本文件，并用目标 Codex CLI 的 model/list 验证。

目录固定版本随 TP 发布，运行时不请求 GitHub 或依赖 Codex OAuth 账户。GET /codex/models.json 在其后追加 codex_extra_models 中已选择且授权可见的模型；不修改官方条目。
