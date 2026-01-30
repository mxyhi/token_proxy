import { useCallback, useEffect, useMemo, useState } from "react";

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
import { PasswordInput } from "@/components/ui/password-input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { m } from "@/paraglide/messages.js";

import type {
  AgentConfig,
  AgentConfigForm,
  AgentSettingsMap,
  AgentToolType,
  CodexReasoningEffort,
  OpenCodeProvider,
} from "./types";
import { AGENT_TOOL_META } from "./constants";
import { createDefaultSettings } from "./agent-store";

type AgentEditorDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 编辑模式时传入现有配置，添加模式时为 null */
  editingAgent: AgentConfig | null;
  /** 添加模式时的默认工具类型 */
  defaultType: AgentToolType;
  onSave: (form: AgentConfigForm) => void;
};

/**
 * Codex 推理强度选项
 */
const CODEX_REASONING_OPTIONS: { value: CodexReasoningEffort; label: string }[] = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

/**
 * OpenCode Provider 选项
 */
const OPENCODE_PROVIDER_OPTIONS: { value: OpenCodeProvider; label: string }[] = [
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Google Gemini" },
  { value: "openrouter", label: "OpenRouter" },
  { value: "custom", label: "Custom" },
];

export function AgentEditorDialog({
  open,
  onOpenChange,
  editingAgent,
  defaultType,
  onSave,
}: AgentEditorDialogProps) {
  // 基础字段
  const [name, setName] = useState("");

  // Settings 字段
  const [apiKey, setApiKey] = useState("");
  const [apiKeyVisible, setApiKeyVisible] = useState(false);
  const [baseUrl, setBaseUrl] = useState("");

  // Claude 特有字段
  const [claudeModel, setClaudeModel] = useState("");
  const [haikuModel, setHaikuModel] = useState("");
  const [sonnetModel, setSonnetModel] = useState("");
  const [opusModel, setOpusModel] = useState("");

  // Codex 特有字段
  const [codexModel, setCodexModel] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<CodexReasoningEffort>("medium");

  // OpenCode 特有字段
  const [openCodeProvider, setOpenCodeProvider] = useState<OpenCodeProvider>("openai");
  const [openCodeModel, setOpenCodeModel] = useState("");

  const isEditing = editingAgent !== null;
  const title = isEditing ? m.agents_editor_title_edit() : m.agents_editor_title_add();
  const currentType = isEditing ? editingAgent.type : defaultType;
  const toolMeta = AGENT_TOOL_META[currentType];

  // 打开对话框时初始化表单
  useEffect(() => {
    if (open) {
      setApiKeyVisible(false);

      if (editingAgent) {
        // 编辑模式：从现有配置初始化
        setName(editingAgent.name);
        const settings = editingAgent.settings;
        setApiKey(settings.apiKey);
        setBaseUrl(settings.baseUrl);

        // 根据类型初始化特有字段
        if (editingAgent.type === "claude") {
          const s = settings as AgentSettingsMap["claude"];
          setClaudeModel(s.model ?? "");
          setHaikuModel(s.haikuModel ?? "");
          setSonnetModel(s.sonnetModel ?? "");
          setOpusModel(s.opusModel ?? "");
        } else if (editingAgent.type === "codex") {
          const s = settings as AgentSettingsMap["codex"];
          setCodexModel(s.model ?? "");
          setReasoningEffort(s.reasoningEffort ?? "medium");
        } else if (editingAgent.type === "opencode") {
          const s = settings as AgentSettingsMap["opencode"];
          setOpenCodeProvider(s.provider ?? "openai");
          setOpenCodeModel(s.model ?? "");
        }
      } else {
        // 添加模式：使用默认值
        setName("");
        const defaultSettings = createDefaultSettings(defaultType);
        setApiKey(defaultSettings.apiKey);
        setBaseUrl(defaultSettings.baseUrl);

        // 重置特有字段
        setClaudeModel("");
        setHaikuModel("");
        setSonnetModel("");
        setOpusModel("");
        setCodexModel("");
        setReasoningEffort("medium");
        setOpenCodeProvider("openai");
        setOpenCodeModel("");
      }
    }
  }, [open, editingAgent, defaultType]);

  // 构建 settings 对象
  const buildSettings = useCallback((): AgentSettingsMap[typeof currentType] => {
    const base = { apiKey, baseUrl };

    switch (currentType) {
      case "claude":
        return {
          ...base,
          ...(claudeModel && { model: claudeModel }),
          ...(haikuModel && { haikuModel }),
          ...(sonnetModel && { sonnetModel }),
          ...(opusModel && { opusModel }),
        } as AgentSettingsMap["claude"];
      case "codex":
        return {
          ...base,
          ...(codexModel && { model: codexModel }),
          reasoningEffort,
        } as AgentSettingsMap["codex"];
      case "opencode":
        return {
          ...base,
          provider: openCodeProvider,
          ...(openCodeModel && { model: openCodeModel }),
        } as AgentSettingsMap["opencode"];
      default:
        return base as AgentSettingsMap[typeof currentType];
    }
  }, [
    currentType,
    apiKey,
    baseUrl,
    claudeModel,
    haikuModel,
    sonnetModel,
    opusModel,
    codexModel,
    reasoningEffort,
    openCodeProvider,
    openCodeModel,
  ]);

  const handleSave = useCallback(() => {
    if (!name.trim()) return;
    onSave({
      name: name.trim(),
      type: currentType,
      settings: buildSettings(),
    });
    onOpenChange(false);
  }, [name, currentType, buildSettings, onSave, onOpenChange]);

  const canSave = useMemo(() => {
    // 名称必填
    if (!name.trim()) return false;
    // API Key 必填
    if (!apiKey.trim()) return false;
    // Base URL 必填
    if (!baseUrl.trim()) return false;
    return true;
  }, [name, apiKey, baseUrl]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="agent-editor-dialog" className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {toolMeta.label()} - {toolMeta.description()}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-6 py-4">
          {/* 基础信息 */}
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="agent-name">{m.agents_editor_name_label()}</Label>
              <Input
                id="agent-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={m.agents_editor_name_placeholder()}
                autoFocus
              />
            </div>
          </div>

          <Separator />

          {/* 连接配置 */}
          <div className="space-y-4">
            <h4 className="text-sm font-medium">{m.agents_editor_connection()}</h4>

            <div className="space-y-2">
              <Label htmlFor="agent-api-key">{m.agents_editor_api_key()}</Label>
              <PasswordInput
                id="agent-api-key"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                visible={apiKeyVisible}
                onVisibilityChange={() => setApiKeyVisible((v) => !v)}
                placeholder={m.agents_editor_api_key_placeholder()}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="agent-base-url">{m.agents_editor_base_url()}</Label>
              <Input
                id="agent-base-url"
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder={m.agents_editor_base_url_placeholder()}
              />
            </div>
          </div>

          <Separator />

          {/* 模型配置 - 根据工具类型动态渲染 */}
          <div className="space-y-4">
            <h4 className="text-sm font-medium">{m.agents_editor_model_config()}</h4>

            {currentType === "claude" && (
              <ClaudeModelFields
                model={claudeModel}
                onModelChange={setClaudeModel}
                haikuModel={haikuModel}
                onHaikuModelChange={setHaikuModel}
                sonnetModel={sonnetModel}
                onSonnetModelChange={setSonnetModel}
                opusModel={opusModel}
                onOpusModelChange={setOpusModel}
              />
            )}

            {currentType === "codex" && (
              <CodexModelFields
                model={codexModel}
                onModelChange={setCodexModel}
                reasoningEffort={reasoningEffort}
                onReasoningEffortChange={setReasoningEffort}
              />
            )}

            {currentType === "opencode" && (
              <OpenCodeModelFields
                provider={openCodeProvider}
                onProviderChange={setOpenCodeProvider}
                model={openCodeModel}
                onModelChange={setOpenCodeModel}
              />
            )}
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

// ============================================================================
// 子组件：各工具类型的模型配置字段
// ============================================================================

type ClaudeModelFieldsProps = {
  model: string;
  onModelChange: (value: string) => void;
  haikuModel: string;
  onHaikuModelChange: (value: string) => void;
  sonnetModel: string;
  onSonnetModelChange: (value: string) => void;
  opusModel: string;
  onOpusModelChange: (value: string) => void;
};

function ClaudeModelFields({
  model,
  onModelChange,
  haikuModel,
  onHaikuModelChange,
  sonnetModel,
  onSonnetModelChange,
  opusModel,
  onOpusModelChange,
}: ClaudeModelFieldsProps) {
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <div className="space-y-2">
        <Label htmlFor="claude-model">{m.agents_editor_claude_model()}</Label>
        <Input
          id="claude-model"
          value={model}
          onChange={(e) => onModelChange(e.target.value)}
          placeholder="claude-sonnet-4-20250514"
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="claude-haiku">{m.agents_editor_claude_haiku()}</Label>
        <Input
          id="claude-haiku"
          value={haikuModel}
          onChange={(e) => onHaikuModelChange(e.target.value)}
          placeholder="claude-haiku-3-5-20241022"
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="claude-sonnet">{m.agents_editor_claude_sonnet()}</Label>
        <Input
          id="claude-sonnet"
          value={sonnetModel}
          onChange={(e) => onSonnetModelChange(e.target.value)}
          placeholder="claude-sonnet-4-20250514"
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="claude-opus">{m.agents_editor_claude_opus()}</Label>
        <Input
          id="claude-opus"
          value={opusModel}
          onChange={(e) => onOpusModelChange(e.target.value)}
          placeholder="claude-opus-4-20250514"
        />
      </div>
    </div>
  );
}

type CodexModelFieldsProps = {
  model: string;
  onModelChange: (value: string) => void;
  reasoningEffort: CodexReasoningEffort;
  onReasoningEffortChange: (value: CodexReasoningEffort) => void;
};

function CodexModelFields({
  model,
  onModelChange,
  reasoningEffort,
  onReasoningEffortChange,
}: CodexModelFieldsProps) {
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <div className="space-y-2">
        <Label htmlFor="codex-model">{m.agents_editor_codex_model()}</Label>
        <Input
          id="codex-model"
          value={model}
          onChange={(e) => onModelChange(e.target.value)}
          placeholder="o3"
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="codex-reasoning">{m.agents_editor_codex_reasoning()}</Label>
        <Select value={reasoningEffort} onValueChange={onReasoningEffortChange}>
          <SelectTrigger id="codex-reasoning">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {CODEX_REASONING_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </div>
  );
}

type OpenCodeModelFieldsProps = {
  provider: OpenCodeProvider;
  onProviderChange: (value: OpenCodeProvider) => void;
  model: string;
  onModelChange: (value: string) => void;
};

function OpenCodeModelFields({
  provider,
  onProviderChange,
  model,
  onModelChange,
}: OpenCodeModelFieldsProps) {
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <div className="space-y-2">
        <Label htmlFor="opencode-provider">{m.agents_editor_opencode_provider()}</Label>
        <Select value={provider} onValueChange={onProviderChange}>
          <SelectTrigger id="opencode-provider">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {OPENCODE_PROVIDER_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-2">
        <Label htmlFor="opencode-model">{m.agents_editor_opencode_model()}</Label>
        <Input
          id="opencode-model"
          value={model}
          onChange={(e) => onModelChange(e.target.value)}
          placeholder="gpt-4o"
        />
      </div>
    </div>
  );
}
