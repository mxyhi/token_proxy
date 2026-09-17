# 生成请求按全局上游优先级调度

同一生成请求的兼容 Upstream 统一按 priority 降序调度，Provider 只决定协议转换和认证；同优先级共享 fill-first/round-robin 与 serial/hedged/race 策略。原先先耗尽 Provider 的做法会让同 Provider 的 99 抢在另一 Provider 的 101 前执行，因此优先级明确优先于原生协议偏好；配置加载时预编排候选，执行时再筛选模型并惰性准备每个 Provider 的出站请求。

原地重试仍固定当前 Upstream/账户，修复后的请求体按 Provider 隔离，跨 Provider 以原始入站请求转换。模型目录、资源操作等专有入口继续服从原生能力约束；Chat 原有的应急 Responses/Codex 桥接只在显式兼容候选耗尽后启用。
