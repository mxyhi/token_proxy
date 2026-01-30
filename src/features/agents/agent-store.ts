import { useCallback, useMemo, useState } from "react";

import type {
  AgentConfig,
  AgentConfigForm,
  AgentSettingsMap,
  AgentToolType,
  ClaudeSettings,
  CodexSettings,
} from "./types";

/**
 * 生成唯一 ID
 */
function generateId() {
  return `agent_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;
}

/**
 * 各工具类型的默认 Base URL
 */
const DEFAULT_BASE_URLS: Record<AgentToolType, string> = {
  claude: "https://api.anthropic.com",
  codex: "https://api.openai.com/v1",
  opencode: "https://api.openai.com/v1",
};

/**
 * 创建默认的 Agent Settings（导出供外部使用）
 */
export function createDefaultSettings<T extends AgentToolType>(type: T): AgentSettingsMap[T] {
  const base = { apiKey: "", baseUrl: DEFAULT_BASE_URLS[type] };

  switch (type) {
    case "claude":
      return { ...base } as AgentSettingsMap[T];
    case "codex":
      return { ...base, reasoningEffort: "medium" } as AgentSettingsMap[T];
    case "opencode":
      return { ...base, provider: "openai" } as AgentSettingsMap[T];
    default:
      return base as AgentSettingsMap[T];
  }
}

/**
 * 创建新的 Agent 配置
 */
function createAgentConfig<T extends AgentToolType>(
  form: AgentConfigForm<T>,
  sortIndex: number
): AgentConfig<T> {
  const now = new Date().toISOString();
  return {
    id: generateId(),
    name: form.name,
    type: form.type,
    settings: form.settings,
    isActive: false,
    sortIndex,
    createdAt: now,
    updatedAt: now,
  };
}

// Mock 初始数据
const INITIAL_AGENTS: AgentConfig[] = [
  {
    id: "agent_default_claude",
    name: "Claude Code (Default)",
    type: "claude",
    settings: {
      apiKey: "",
      baseUrl: "https://api.anthropic.com",
    } as ClaudeSettings,
    isActive: true,
    sortIndex: 0,
    createdAt: "2025-01-01T00:00:00.000Z",
    updatedAt: "2025-01-01T00:00:00.000Z",
  },
  {
    id: "agent_default_codex",
    name: "Codex CLI",
    type: "codex",
    settings: {
      apiKey: "",
      baseUrl: "https://api.openai.com/v1",
      reasoningEffort: "medium",
    } as CodexSettings,
    isActive: false,
    sortIndex: 1,
    createdAt: "2025-01-01T00:00:00.000Z",
    updatedAt: "2025-01-01T00:00:00.000Z",
  },
];

/**
 * Agent 配置状态管理 Hook
 */
export function useAgentStore() {
  const [agents, setAgents] = useState<AgentConfig[]>(INITIAL_AGENTS);

  // 按 sortIndex 排序的 agents
  const sortedAgents = useMemo(
    () => [...agents].sort((a, b) => a.sortIndex - b.sortIndex),
    [agents]
  );

  // 当前激活的 agent
  const activeAgent = useMemo(
    () => agents.find((agent) => agent.isActive) ?? null,
    [agents]
  );

  // 按类型分组
  const agentsByType = useMemo(() => {
    const grouped: Record<AgentToolType, AgentConfig[]> = {
      claude: [],
      codex: [],
      opencode: [],
    };
    for (const agent of sortedAgents) {
      grouped[agent.type].push(agent);
    }
    return grouped;
  }, [sortedAgents]);

  // 添加 agent
  const addAgent = useCallback((form: AgentConfigForm) => {
    setAgents((prev) => {
      const maxSortIndex = prev.reduce((max, a) => Math.max(max, a.sortIndex), -1);
      const newAgent = createAgentConfig(form, maxSortIndex + 1);
      // 如果是第一个该类型的 agent，自动激活
      const hasActiveOfType = prev.some((a) => a.type === form.type && a.isActive);
      if (!hasActiveOfType) {
        newAgent.isActive = true;
      }
      return [...prev, newAgent];
    });
  }, []);

  // 更新 agent（包括 settings）
  const updateAgent = useCallback((id: string, form: AgentConfigForm) => {
    setAgents((prev) =>
      prev.map((agent) =>
        agent.id === id
          ? {
              ...agent,
              name: form.name,
              type: form.type,
              settings: form.settings,
              updatedAt: new Date().toISOString(),
            }
          : agent
      )
    );
  }, []);

  // 删除 agent
  const deleteAgent = useCallback((id: string) => {
    setAgents((prev) => {
      const target = prev.find((a) => a.id === id);
      if (!target) return prev;

      const remaining = prev.filter((a) => a.id !== id);

      // 如果删除的是激活的 agent，激活同类型的第一个
      if (target.isActive) {
        const sameType = remaining.filter((a) => a.type === target.type);
        if (sameType.length > 0) {
          const firstOfType = sameType.sort((a, b) => a.sortIndex - b.sortIndex)[0];
          return remaining.map((a) =>
            a.id === firstOfType.id ? { ...a, isActive: true } : a
          );
        }
      }

      return remaining;
    });
  }, []);

  // 切换激活状态（同类型只能有一个激活）
  const switchAgent = useCallback((id: string) => {
    setAgents((prev) => {
      const target = prev.find((a) => a.id === id);
      if (!target || target.isActive) return prev;

      return prev.map((agent) => {
        // 同类型的其他 agent 取消激活
        if (agent.type === target.type) {
          return { ...agent, isActive: agent.id === id };
        }
        return agent;
      });
    });
  }, []);

  // 更新排序
  const reorderAgents = useCallback((reorderedIds: string[]) => {
    setAgents((prev) => {
      const idToAgent = new Map(prev.map((a) => [a.id, a]));
      return reorderedIds.map((id, index) => {
        const agent = idToAgent.get(id);
        if (!agent) throw new Error(`Agent not found: ${id}`);
        return { ...agent, sortIndex: index };
      });
    });
  }, []);

  return {
    agents: sortedAgents,
    activeAgent,
    agentsByType,
    addAgent,
    updateAgent,
    deleteAgent,
    switchAgent,
    reorderAgents,
  };
}

export type AgentStore = ReturnType<typeof useAgentStore>;
