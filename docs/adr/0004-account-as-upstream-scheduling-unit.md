# 账户迁入 Upstream，Upstream 为唯一调度单元

Kiro / Codex / xAI 账户不再作为可独立调度的 Provider 条目：`priority`、`proxy_url`、`enabled`、模型限制/映射与 credential 只属于 Upstream；全局 `upstream_strategy` / `same_upstream_retry_count` 以 Upstream 为唯一候选/调度单元作用，并非单条 Upstream 字段，亦不宣称 per-upstream retry 或 per-upstream dispatch/order 已实现。账户只持久化认证身份（及适用时的配额、token 刷新）。登录或导入由应用生命周期保证「1 account ↔ 1 稳定 Upstream」的 binding，前端不得再构造 `*-default` 占位 Upstream。删除 Account-backed Upstream 级联删除对应账户凭据；复制该类 Upstream 被禁止，避免同一 credential identity 被多条路由绑定。

我们拒绝「账户表继续带优先级/代理/开关」与「配置层双写旧平铺 `*_account_id` + 独立 Accounts 页」：前者让调度真相分裂，后者让 UI 与 config 两套模型并存。Trade-off：级联删除更陡、账户侧字段更少，但调度、配置与 UI 只剩一条 Upstream 主线。
