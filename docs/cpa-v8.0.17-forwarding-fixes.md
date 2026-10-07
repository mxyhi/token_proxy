# CPA v8.0.17 转发边界修复

本次以 Token Proxy v0.1.208 和 CPA v8.0.17 为审查基线，修复本地已复现的转发、内存和凭据并发边界。这些改动纳入 v0.1.209；本文验证范围为源码与本地模拟上游测试，不包含安装后或真实供应商验收。

## 行为变化

- 原生 Responses 流未收到结束标记便 EOF 时，向客户端输出 `response.failed` 并记录 502。已经提交 HTTP 200 的流不能改写 HTTP 状态；不会自动重放已输出内容。
- 所有改写 SSE 的路径移除上游 `Content-Length`，由 HTTP 层重新编码。请求与响应都过滤 `Connection` 指定的逐跳字段。
- `max_request_body_bytes` 在实际读取层执行，超限返回 413；固定长度、无长度和 chunked 上传使用相同预算。鉴权失败的详情捕获也受限。
- SSE 单行、合并后的 data 正文上限为 50 MiB，raw Chat 帧缓冲也受限。超过预算会明确失败并释放接收缓存，不会静默截断为成功。多个合法事件组成的流没有总长度上限。
- 关闭 Request Detail 后不再为日志保留完整 SSE。开启时小正文留在内存，超过 256 KiB 转匿名临时文件，再以 64 KiB 块落库；详情按页查看和复制，不引入隐式丢日志。原生 Responses 遇到非 SSE HTTP 200 时，只保留至多 64 KiB 诊断前缀；响应明确失败，前缀截断会标注。
- Kiro 同账户并发刷新合并，取消释放刷新状态；网络完成后重读当前凭据，仅保存仍有效的结果。删除账户不会被旧刷新恢复，重新登录和新额度缓存不会被旧快照覆盖。
- Codex 明确收到 401 后不再向并发请求返回同一被拒绝 token。主动刷新失败也不能回退到该 token；额度查询和 Authorization 构造本身不会新增 OAuth exchange。
- 迟到成功不清掉其他请求已建立的冷却。Retry-After 和供应商明确额度等待不会被同上游重试或会话收尾解除；普通短暂错误保留既有 scope 清理行为。
- HTTP、prelude 和 transport 结果使用实际发送的凭据，在账户缓存读锁内检查后更新可用性；旧凭据失败仍返回并落日志，但不能冷却新凭据。Agent Identity 对照实际 assertion 的身份与任务；xAI 被动额度头在落库前也检查发送凭据。

## 回归与性能边界

第一轮边界修复的 `cargo test --workspace` 通过全部 1,237 项测试，`cargo fmt --all -- --check` 与 `git diff --check` 通过。重点回归位于：

- `proxy/server/body_limit.test.rs`：真实 router/socket 的 fixed/chunked 请求，精确上限可通过，超限 413；读取失败不会继续消费尾部。
- `proxy/response/tests_forwarding_contract.rs`：原生 Responses EOF 错误及 SQLite 记录、真实上下游 socket 的长度编码、关闭详情无全文保留与开启详情完整记录、有界诊断、SSE 超限。
- `token_proxy_protocol/src/sse.rs`：跨网络分片、多行事件累计、UTF-8、大 transport chunk 多个合法事件与大工具参数。
- `proxy/server/credential_results.test.rs`：本地上游请求在途时替换凭据，迟到 429 不改变新凭据/上游可用性，真实错误仍落库。
- 账户刷新测试：Kiro single-flight、取消、删除/重新登录；Codex 401 并发拒绝、禁止不安全回退；Agent Identity 旧任务 assertion；xAI 旧凭据额度头。

后续已补齐完整详情的有界捕获、分块落库与分页读取。用户选择完整性优先：慢磁盘等待写入，写入失败继续转发且明确记录捕获错误；正常客户端取消时移交捕获状态，日志任务刷完缓冲再提交。超过七天的正文块与原详情同步清理。非法 UTF-8 原字节保留，无法作为文本显示时明确报错；正文块缺失、长度异常或编码非法时不伪装成完整详情。

本轮新增临时文件直接依赖 `tempfile`（版本已在 Cargo.lock 中），避免自行处理敏感临时文件的命名、权限及退出清理。详情 API 的 `responseBody` 变为首页，新增总字节数、下一页偏移与捕获错误；大正文存入 `response_body_chunks`，旧 `response_body` 不再代表新日志的完整正文。旧大 TEXT 行兼容分页，但 SQLite 内部读取仍可能加载全文。Kiro 和其他协议转换为终态合成、用量估计保留的协议状态，以及原请求体缓存，不属于日志缓冲的恒定内存承诺。

新增验证覆盖真实流取消后的正文前缀完整性、跨页 UTF-8、文件写入失败、读取中断、插块事务失败回滚、缺块和七天清理；UI 覆盖翻页、复制范围、错误展示和异步返回竞态。性能使用本地真实 runtime/socket/SQLite 与模拟上游，不代表真实供应商或安装后表现。详见 [性能验证](cpa-v8.0.17-performance.md) 和 [存储决策](adr/0006-bounded-response-detail.md)。

原有 `DEFAULT_JSON_TRANSFORM_LIMIT_BYTES` 在非测试编译中有 dead_code 警告，与本次修改无关。

完整详情治理追加后，`cargo test --workspace` 通过 1,251 项，前端 41 个文件、232 项测试通过；TypeScript 检查与 Vite build 通过。现有 ESLint 依赖与 TypeScript 7 的兼容问题导致 ESLint 无法运行，未更改依赖规避此限制。
