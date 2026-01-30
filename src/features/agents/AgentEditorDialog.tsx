import { useCallback, useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { m } from "@/paraglide/messages.js";

import type { AgentConfig, AgentConfigForm, AgentToolType } from "./types";
import { AGENT_TOOL_META } from "./constants";

type AgentEditorDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 编辑模式时传入现有配置，添加模式时为 null */
  editingAgent: AgentConfig | null;
  /** 添加模式时的默认工具类型 */
  defaultType: AgentToolType;
  onSave: (form: AgentConfigForm) => void;
};

export function AgentEditorDialog({
  open,
  onOpenChange,
  editingAgent,
  defaultType,
  onSave,
}: AgentEditorDialogProps) {
  const [name, setName] = useState("");

  const isEditing = editingAgent !== null;
  const title = isEditing ? m.agents_editor_title_edit() : m.agents_editor_title_add();
  const currentType = isEditing ? editingAgent.type : defaultType;
  const toolMeta = AGENT_TOOL_META[currentType];

  // 打开对话框时初始化表单
  useEffect(() => {
    if (open) {
      if (editingAgent) {
        setName(editingAgent.name);
      } else {
        setName("");
      }
    }
  }, [open, editingAgent]);

  const handleNameChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    setName(e.target.value);
  }, []);

  const handleSave = useCallback(() => {
    if (!name.trim()) return;
    onSave({ name: name.trim(), type: currentType });
    onOpenChange(false);
  }, [name, currentType, onSave, onOpenChange]);

  const canSave = name.trim().length > 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="agent-editor-dialog" className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {toolMeta.label()} - {toolMeta.description()}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {/* 名称 */}
          <div className="space-y-2">
            <Label htmlFor="agent-name">{m.agents_editor_name_label()}</Label>
            <Input
              id="agent-name"
              value={name}
              onChange={handleNameChange}
              placeholder={m.agents_editor_name_placeholder()}
              autoFocus
            />
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {m.common_cancel()}
          </Button>
          <Button onClick={handleSave} disabled={!canSave}>
            {m.common_save()}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
