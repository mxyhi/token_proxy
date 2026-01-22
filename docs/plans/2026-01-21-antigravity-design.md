# Antigravity 功能接入设计（token_proxy）

## 目标
在 token_proxy 中实现 Antigravity 全量支持：OAuth 登录/刷新、IDE 导入与账号切换、配额/订阅、warmup、代理 provider 与格式转换、Providers UI 集成；跨平台可配置，macOS 提供默认路径。

## 架构概览
后端新增 `src-tauri/src/antigravity/` 模块，负责账号体系与 IDE 交互；代理层新增 antigravity provider（以 Gemini 格式为中间层）；前端新增 Providers 分组与 Upstreams 账号选择。

### 后端模块划分
- `antigravity/types.rs`: 账号、登录、配额、IDE 状态、warmup 调度的数据结构。
- `antigravity/oauth.rs`: OAuth URL 构建、token 交换与刷新、userinfo 获取。
- `antigravity/login.rs`: 本地回调监听 + 登录会话轮询。
- `antigravity/store.rs`: token 文件存储（config_dir/antigravity-auth），过期刷新。
- `antigravity/ide_db.rs`: SQLite 读写、备份/回滚、WAL/SHM 清理、active email 读取。
- `antigravity/protobuf.rs`: protobuf field 6 注入与抽取（access/refresh/expiry）。
- `antigravity/ide.rs`: IDE 导入与账号切换流程（终止/注入/重启）。
- `antigravity/quota.rs`: fetchAvailableModels + loadCodeAssist（project id/plan）。
- `antigravity/warmup.rs`: 手动 warmup + 轻量调度（interval）。

### 代理层接入
- 新增 provider `antigravity`。
- 请求转换：Chat/Responses/Anthropic → Gemini（现有转换）；Gemini → Antigravity wrapper（新增）。
- 响应转换：Antigravity（Gemini 格式）→ Chat/Responses/Anthropic（复用现有 Gemini 响应转换）。
- 上游请求：使用 OAuth access_token；遇 401 自动刷新；base URL 按 daily/sandbox/prod 回退。

### 配置与跨平台
- macOS 默认 IDE DB 路径与进程名来自 quotio。
- Windows/Linux 允许通过 `proxy_config` 的 antigravity 配置覆盖 IDE DB 路径、进程名、应用路径；未配置则 IDE 功能降级提示。

## 关键流程
1. OAuth 登录：生成 state → 监听本地回调 → exchange token → userinfo(email) → 保存 token record。
2. IDE 导入：读取 state.vscdb → protobuf 抽取 access/refresh/expiry → 保存账号记录。
3. IDE 切换：终止 IDE → 备份 DB → 注入新 token → 重启 IDE → 清理备份；失败回滚。
4. 代理请求：将请求转换为 Gemini → Antigravity wrapper → 上游发送 → Gemini 响应转换回客户端格式。
5. 配额：loadCodeAssist 获取 project id 与订阅 tier → fetchAvailableModels → 组装 quota 列表。
6. Warmup：对指定 account+model 执行 generateContent（maxOutputTokens=1）；可按 interval 定时。

## 错误处理
- DB 锁/超时：busy_timeout + 重试；失败回滚备份。
- OAuth 刷新失败：账号标记过期并在 UI 提示。
- IDE 未安装：返回明确错误，UI 仅显示导入/切换不可用。

## 测试策略
- Rust：protobuf 注入/抽取单测；IDE DB 读写 mock（若需要）。
- TS：类型检查 + Providers UI 基础交互。
