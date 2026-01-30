import { useCallback, useEffect, useState } from "react";
import { Bot } from "lucide-react";

import { m } from "@/paraglide/messages.js";

import type { AgentConfig, AgentConfigForm, AgentToolType } from "./types";
import { useAgentStore } from "./agent-store";
import { AgentCard } from "./AgentCard";
import { AgentEditorDialog } from "./AgentEditorDialog";
import { AgentDeleteDialog } from "./AgentDeleteDialog";

type AgentsPanelProps = {
  /** 当前选中的工具类型 */
  selectedTool: AgentToolType;
  /** 添加对话框触发器（每次变化时打开添加对话框） */
  addTrigger: number;
};

export function AgentsPanel({ selectedTool, addTrigger }: AgentsPanelProps) {
  const store = useAgentStore();

  // 编辑对话框状态
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingAgent, setEditingAgent] = useState<AgentConfig | null>(null);

  // 删除对话框状态
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deletingAgent, setDeletingAgent] = useState<AgentConfig | null>(null);

  // 根据 selectedTool 过滤 agents
  const filteredAgents = store.agents.filter((agent) => agent.type === selectedTool);

  // 监听 addTrigger 变化，打开添加对话框
  useEffect(() => {
    if (addTrigger > 0) {
      setEditingAgent(null);
      setEditorOpen(true);
    }
  }, [addTrigger]);

  // 打开编辑对话框
  const handleEdit = useCallback((agent: AgentConfig) => {
    setEditingAgent(agent);
    setEditorOpen(true);
  }, []);

  // 保存（添加或更新）
  const handleSave = useCallback(
    (form: AgentConfigForm) => {
      if (editingAgent) {
        store.updateAgent(editingAgent.id, form);
      } else {
        // 添加时使用当前选中的工具类型
        store.addAgent({ ...form, type: selectedTool });
      }
    },
    [editingAgent, store, selectedTool]
  );

  // 打开删除确认对话框
  const handleDeleteClick = useCallback((id: string) => {
    const agent = store.agents.find((a) => a.id === id);
    if (agent) {
      setDeletingAgent(agent);
      setDeleteOpen(true);
    }
  }, [store.agents]);

  // 确认删除
  const handleDeleteConfirm = useCallback(() => {
    if (deletingAgent) {
      store.deleteAgent(deletingAgent.id);
      setDeleteOpen(false);
      setDeletingAgent(null);
    }
  }, [deletingAgent, store]);

  const isEmpty = filteredAgents.length === 0;

  return (
    <div data-slot="agents-panel">
      {isEmpty ? (
        // 空状态
        <div className="flex flex-col items-center justify-center py-12 text-center">
          <div className="mb-4 rounded-full bg-muted p-4">
            <Bot className="size-8 text-muted-foreground" />
          </div>
          <p className="text-sm font-medium">{m.agents_empty()}</p>
          <p className="mt-1 text-xs text-muted-foreground">{m.agents_empty_hint()}</p>
        </div>
      ) : (
        // Agent 列表
        <div className="space-y-3">
          {filteredAgents.map((agent) => (
            <AgentCard
              key={agent.id}
              agent={agent}
              onSwitch={store.switchAgent}
              onEdit={handleEdit}
              onDelete={handleDeleteClick}
            />
          ))}
        </div>
      )}

      {/* 编辑对话框 */}
      <AgentEditorDialog
        open={editorOpen}
        onOpenChange={setEditorOpen}
        editingAgent={editingAgent}
        defaultType={selectedTool}
        onSave={handleSave}
      />

      {/* 删除确认对话框 */}
      <AgentDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        agentName={deletingAgent?.name ?? ""}
        onConfirm={handleDeleteConfirm}
      />
    </div>
  );
}
