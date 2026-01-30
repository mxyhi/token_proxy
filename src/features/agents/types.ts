/**
 * Agent 工具类型
 */
export type AgentToolType = "claude" | "codex" | "opencode";

/**
 * Agent 配置项
 */
export type AgentConfig = {
  /** 唯一标识符 */
  id: string;
  /** 配置名称 */
  name: string;
  /** Agent 工具类型 */
  type: AgentToolType;
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
 * Agent 配置表单
 */
export type AgentConfigForm = {
  name: string;
  type: AgentToolType;
};

/**
 * Agent 工具元数据
 */
export type AgentToolMeta = {
  type: AgentToolType;
  label: () => string;
  description: () => string;
};
