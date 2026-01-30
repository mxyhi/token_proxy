/**
 * Agent 工具类型
 */
export type AgentToolType = "claude" | "codex" | "opencode";

// ============================================================================
// Agent Settings 类型定义
// ============================================================================

/**
 * 通用连接配置
 */
type BaseAgentSettings = {
  /** API 密钥 */
  apiKey: string;
  /** API 基础 URL */
  baseUrl: string;
};

/**
 * Claude 特有配置
 */
export type ClaudeSettings = BaseAgentSettings & {
  /** 主模型（用于一般任务） */
  model?: string;
  /** Haiku 模型（快速轻量任务） */
  haikuModel?: string;
  /** Sonnet 模型（平衡任务） */
  sonnetModel?: string;
  /** Opus 模型（复杂任务） */
  opusModel?: string;
};

/**
 * Codex 推理强度
 */
export type CodexReasoningEffort = "low" | "medium" | "high";

/**
 * Codex 特有配置
 */
export type CodexSettings = BaseAgentSettings & {
  /** 模型名称 */
  model?: string;
  /** 推理强度 */
  reasoningEffort?: CodexReasoningEffort;
};

/**
 * OpenCode Provider 类型
 */
export type OpenCodeProvider = "openai" | "anthropic" | "gemini" | "openrouter" | "custom";

/**
 * OpenCode 特有配置
 */
export type OpenCodeSettings = BaseAgentSettings & {
  /** Provider 类型 */
  provider?: OpenCodeProvider;
  /** 模型名称 */
  model?: string;
};

/**
 * Agent 工具类型到配置的映射
 */
export type AgentSettingsMap = {
  claude: ClaudeSettings;
  codex: CodexSettings;
  opencode: OpenCodeSettings;
};

/**
 * 所有 Agent Settings 的联合类型
 */
export type AgentSettings = AgentSettingsMap[AgentToolType];

// ============================================================================
// Agent Config 类型定义
// ============================================================================

/**
 * Agent 配置项
 */
export type AgentConfig<T extends AgentToolType = AgentToolType> = {
  /** 唯一标识符 */
  id: string;
  /** 配置名称 */
  name: string;
  /** Agent 工具类型 */
  type: T;
  /** 工具特定配置 */
  settings: AgentSettingsMap[T];
  /** 是否为当前激活配置 */
  isActive: boolean;
  /** 排序索引（越小越靠前） */
  sortIndex: number;
  /** 创建时间 ISO 字符串 */
  createdAt: string;
  /** 更新时间 ISO 字符串 */
  updatedAt: string;
};

/**
 * Agent 配置表单（用于添加/编辑）
 */
export type AgentConfigForm<T extends AgentToolType = AgentToolType> = {
  name: string;
  type: T;
  settings: AgentSettingsMap[T];
};

/**
 * Agent 工具元数据
 */
export type AgentToolMeta = {
  type: AgentToolType;
  label: () => string;
  description: () => string;
};

// ============================================================================
// 工具函数类型
// ============================================================================

/**
 * 创建默认 settings 的工厂函数类型
 */
export type CreateDefaultSettings = <T extends AgentToolType>(type: T) => AgentSettingsMap[T];
