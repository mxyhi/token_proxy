import { m } from "@/paraglide/messages.js";

import type { AgentToolMeta, AgentToolType } from "./types";

/**
 * Agent 工具元数据
 */
export const AGENT_TOOL_META: Record<AgentToolType, AgentToolMeta> = {
  claude: {
    type: "claude",
    label: () => m.agents_tool_claude(),
    description: () => m.agents_tool_claude_desc(),
  },
  codex: {
    type: "codex",
    label: () => m.agents_tool_codex(),
    description: () => m.agents_tool_codex_desc(),
  },
  opencode: {
    type: "opencode",
    label: () => m.agents_tool_opencode(),
    description: () => m.agents_tool_opencode_desc(),
  },
};

/**
 * Agent 工具类型列表（用于下拉选择）
 */
export const AGENT_TOOL_OPTIONS: AgentToolType[] = ["claude", "codex", "opencode"];
