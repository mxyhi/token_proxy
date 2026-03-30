import { CirclePlus, HelpCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
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
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { ConvertFromMapEditor } from "@/features/config/cards/upstreams/convert-from-map-editor";
import {
  EditorField,
  HeaderOverridesEditor,
  ModelMappingsEditor,
} from "@/features/config/cards/upstreams/editor-fields";
import { ProviderMultiSelect } from "@/features/config/cards/upstreams/provider-multi-select";
import { createModelMapping } from "@/features/config/form";
import type {
  HeaderOverrideForm,
  KiroPreferredEndpoint,
  UpstreamForm,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

const KIRO_ENDPOINT_INHERIT = "inherit";

const KIRO_ENDPOINT_OPTIONS: ReadonlyArray<{
  value: KiroPreferredEndpoint | typeof KIRO_ENDPOINT_INHERIT;
  label: () => string;
}> = [
  { value: KIRO_ENDPOINT_INHERIT, label: () => m.kiro_preferred_endpoint_inherit() },
  { value: "ide", label: () => m.kiro_preferred_endpoint_ide() },
  { value: "cli", label: () => m.kiro_preferred_endpoint_cli() },
];

function isKiroPreferredEndpoint(value: string): value is KiroPreferredEndpoint {
  return value === "ide" || value === "cli";
}

function isLockedAccountBackedUpstream(draft: UpstreamForm) {
  const providers = draft.providers.map((value) => value.trim()).filter(Boolean);
  return (
    providers.length === 1 &&
    ((providers[0] === "kiro" && draft.id.trim() === "kiro-default") ||
      (providers[0] === "codex" && draft.id.trim() === "codex-default"))
  );
}

export type UpstreamEditorFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  appProxyUrl: string;
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

type UpstreamIdentityFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  appProxyUrl: string;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
  showBaseUrl: boolean;
  showProxyUrl: boolean;
};

function UpstreamIdentityFields({
  draft,
  providerOptions,
  appProxyUrl,
  onChangeDraft,
  showBaseUrl,
  showProxyUrl,
}: UpstreamIdentityFieldsProps) {
  const canUseAppProxy = !!appProxyUrl.trim();
  const providers = draft.providers.map((value) => value.trim()).filter(Boolean);
  const isKiro = providers.includes("kiro");
  const isLocked = isLockedAccountBackedUpstream(draft);
  const kiroEndpointValue = draft.preferredEndpoint.trim()
    ? draft.preferredEndpoint
    : KIRO_ENDPOINT_INHERIT;

  return (
    <div data-slot="upstream-identity-fields" className="contents">
      {/* 第一行：Provider 和 Status 并排 */}
      <Label className="inline-flex items-center gap-1">
        {m.field_provider()}
        <Tooltip>
          <TooltipTrigger asChild>
            <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
          </TooltipTrigger>
          <TooltipContent side="right" className="max-w-xs">
            {m.field_provider_tip()}
          </TooltipContent>
        </Tooltip>
      </Label>
      <div className="grid grid-cols-[1fr_auto_1fr] items-center gap-3">
        <ProviderMultiSelect
          providerOptions={providerOptions}
          value={draft.providers}
          disabled={isLocked}
          onChange={(next) => onChangeDraft({ providers: next })}
        />
        <Label className="inline-flex items-center gap-1">
          {m.field_status()}
          <Tooltip>
            <TooltipTrigger asChild>
              <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-xs">
              {m.field_status_tip()}
            </TooltipContent>
          </Tooltip>
        </Label>
        <Select
          value={draft.enabled ? "enabled" : "disabled"}
          onValueChange={(value) =>
            onChangeDraft({ enabled: value === "enabled" })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="enabled">{m.common_enabled()}</SelectItem>
            <SelectItem value="disabled">{m.common_disabled()}</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {isKiro ? (
        <EditorField
          label={m.field_kiro_preferred_endpoint()}
          tooltip={m.field_kiro_preferred_endpoint_tip()}
          htmlFor="upstream-editor-kiro-endpoint"
        >
          <Select
            value={kiroEndpointValue}
            onValueChange={(value) => {
              if (value === KIRO_ENDPOINT_INHERIT) {
                onChangeDraft({ preferredEndpoint: "" });
                return;
              }
              if (isKiroPreferredEndpoint(value)) {
                onChangeDraft({ preferredEndpoint: value });
              }
            }}
          >
            <SelectTrigger id="upstream-editor-kiro-endpoint">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KIRO_ENDPOINT_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </EditorField>
      ) : null}

      {/* 第三行：ID（自动生成，可编辑） */}
      <EditorField label={m.field_id()} tooltip={m.field_id_tip()} htmlFor="upstream-editor-id">
        <Input
          id="upstream-editor-id"
          value={draft.id}
          disabled={isLocked}
          onChange={(e) => onChangeDraft({ id: e.target.value })}
          placeholder="openai-default"
        />
      </EditorField>

      {showBaseUrl ? (
        <EditorField
          label={m.field_base_url()}
          tooltip={m.field_base_url_tip()}
          htmlFor="upstream-editor-baseUrl"
        >
          <Input
            id="upstream-editor-baseUrl"
            value={draft.baseUrl}
            onChange={(e) => onChangeDraft({ baseUrl: e.target.value })}
            placeholder="https://api.openai.com"
          />
        </EditorField>
      ) : null}

      {showProxyUrl ? (
        <EditorField
          label={m.field_proxy_url()}
          tooltip={m.upstreams_proxy_tip({ placeholder: "$app_proxy_url" })}
          htmlFor="upstream-editor-proxyUrl"
        >
          <div className="flex items-center gap-2">
            <Input
              id="upstream-editor-proxyUrl"
              value={draft.proxyUrl}
              onChange={(e) => onChangeDraft({ proxyUrl: e.target.value })}
              placeholder="http://127.0.0.1:7890"
              className="flex-1"
            />
            {canUseAppProxy ? (
              <Button
                type="button"
                size="sm"
                variant="secondary"
                onClick={() => onChangeDraft({ proxyUrl: "$app_proxy_url" })}
              >
                {m.upstreams_proxy_use_app()}
              </Button>
            ) : null}
          </div>
        </EditorField>
      ) : null}
    </div>
  );
}

type UpstreamAuthFieldsProps = {
  draft: UpstreamForm;
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamAuthFields({
  draft,
  showApiKeys,
  onToggleApiKeys,
  onChangeDraft,
}: UpstreamAuthFieldsProps) {
  return (
    <div data-slot="upstream-auth-fields" className="contents">
      <EditorField
        label={m.field_api_key()}
        tooltip={m.field_api_key_tip()}
        htmlFor="upstream-editor-apiKeys"
      >
        <PasswordInput
          id="upstream-editor-apiKeys"
          visible={showApiKeys}
          onVisibilityChange={onToggleApiKeys}
          value={draft.apiKeys}
          onChange={(e) => onChangeDraft({ apiKeys: e.target.value })}
          placeholder={m.common_optional()}
        />
      </EditorField>
    </div>
  );
}

type UpstreamOpenAIResponsesFieldsProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamOpenAIResponsesFields({
  draft,
  onChangeDraft,
}: UpstreamOpenAIResponsesFieldsProps) {
  const isOpenai = draft.providers.some((value) => value.trim() === "openai");
  const isOpenaiResponses = draft.providers.some((value) => value.trim() === "openai-response");
  if (!isOpenai && !isOpenaiResponses) {
    return null;
  }

  return (
    <div data-slot="upstream-openai-responses-fields" className="contents">
      {isOpenaiResponses ? (
        <div className="col-span-2 flex items-center gap-3">
          <Label className="inline-flex items-center gap-1">
            {m.field_use_chat_completions_for_responses()}
            <Tooltip>
              <TooltipTrigger asChild>
                <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
              </TooltipTrigger>
              <TooltipContent side="right" className="max-w-xs">
                {m.field_use_chat_completions_for_responses_tip()}
              </TooltipContent>
            </Tooltip>
          </Label>
          <Switch
            checked={draft.useChatCompletionsForResponses}
            onCheckedChange={(checked) =>
              onChangeDraft({ useChatCompletionsForResponses: checked })
            }
            aria-label={m.field_use_chat_completions_for_responses_aria()}
          />
        </div>
      ) : null}
      {isOpenaiResponses ? (
        <>
          <div className="col-span-2 flex items-center gap-3">
            <Label className="inline-flex items-center gap-1">
              {m.field_filter_prompt_cache_retention()}
              <Tooltip>
                <TooltipTrigger asChild>
                  <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
                </TooltipTrigger>
                <TooltipContent side="right" className="max-w-xs">
                  {m.field_filter_prompt_cache_retention_tip()}
                </TooltipContent>
              </Tooltip>
            </Label>
            <Switch
              checked={draft.filterPromptCacheRetention}
              onCheckedChange={(checked) =>
                onChangeDraft({ filterPromptCacheRetention: checked })
              }
              aria-label={m.field_filter_prompt_cache_retention_aria()}
            />
          </div>
          <div className="col-span-2 flex items-center gap-3">
            <Label className="inline-flex items-center gap-1">
              {m.field_filter_safety_identifier()}
              <Tooltip>
                <TooltipTrigger asChild>
                  <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
                </TooltipTrigger>
                <TooltipContent side="right" className="max-w-xs">
                  {m.field_filter_safety_identifier_tip()}
                </TooltipContent>
              </Tooltip>
            </Label>
            <Switch
              checked={draft.filterSafetyIdentifier}
              onCheckedChange={(checked) => onChangeDraft({ filterSafetyIdentifier: checked })}
              aria-label={m.field_filter_safety_identifier_aria()}
            />
          </div>
        </>
      ) : null}
      <div className="col-span-2 flex items-center gap-3">
        <Label className="inline-flex items-center gap-1">
          {m.field_rewrite_developer_role_to_system()}
          <Tooltip>
            <TooltipTrigger asChild>
              <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-xs">
              {m.field_rewrite_developer_role_to_system_tip()}
            </TooltipContent>
          </Tooltip>
        </Label>
        <Switch
          checked={draft.rewriteDeveloperRoleToSystem}
          onCheckedChange={(checked) => onChangeDraft({ rewriteDeveloperRoleToSystem: checked })}
          aria-label={m.field_rewrite_developer_role_to_system_aria()}
        />
      </div>
    </div>
  );
}


type UpstreamOrderFieldsProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamOrderFields({ draft, onChangeDraft }: UpstreamOrderFieldsProps) {
  return (
    <div data-slot="upstream-order-fields" className="contents">
      <EditorField
        label={m.field_priority()}
        tooltip={m.field_priority_tip()}
        htmlFor="upstream-editor-priority"
      >
        <Input
          id="upstream-editor-priority"
          value={draft.priority}
          onChange={(e) => onChangeDraft({ priority: e.target.value })}
          placeholder="0"
          inputMode="numeric"
        />
      </EditorField>
    </div>
  );
}

type UpstreamModelMappingFieldsProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamModelMappingFields({
  draft,
  onChangeDraft,
}: UpstreamModelMappingFieldsProps) {
  const handleAdd = () => {
    onChangeDraft({ modelMappings: [...draft.modelMappings, createModelMapping()] });
  };

  return (
    <div data-slot="upstream-model-mapping-fields" className="col-span-2 space-y-2">
      <div className="flex items-center gap-2">
        <Label className="inline-flex items-center gap-1">
          {m.field_model_mappings()}
          <Tooltip>
            <TooltipTrigger asChild>
              <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-xs">
              {m.model_mappings_tip()}
            </TooltipContent>
          </Tooltip>
        </Label>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={m.model_mappings_add()}
          onClick={handleAdd}
        >
          <CirclePlus className="size-4" aria-hidden="true" />
        </Button>
      </div>
      {draft.modelMappings.length > 0 ? (
        <ModelMappingsEditor
          mappings={draft.modelMappings}
          onChange={(next) => onChangeDraft({ modelMappings: next })}
        />
      ) : null}
    </div>
  );
}

type UpstreamHeaderOverrideFieldsProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamHeaderOverrideFields({
  draft,
  onChangeDraft,
}: UpstreamHeaderOverrideFieldsProps) {
  const handleAdd = () => {
    const next: HeaderOverrideForm = {
      id: `header-override-${Date.now()}-${draft.overrides.header.length}`,
      name: "",
      value: "",
      isNull: false,
    };
    onChangeDraft({ overrides: { header: [...draft.overrides.header, next] } });
  };

  return (
    <div data-slot="upstream-header-override-fields" className="col-span-2 space-y-2">
      <div className="flex items-center gap-2">
        <Label className="inline-flex items-center gap-1">
          {m.field_header_overrides()}
          <Tooltip>
            <TooltipTrigger asChild>
              <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-xs">
              {m.header_overrides_tip()}
            </TooltipContent>
          </Tooltip>
        </Label>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={m.header_overrides_add()}
          onClick={handleAdd}
        >
          <CirclePlus className="size-4" aria-hidden="true" />
        </Button>
      </div>
      {draft.overrides.header.length > 0 ? (
        <HeaderOverridesEditor
          overrides={draft.overrides.header}
          onChange={(header) => onChangeDraft({ overrides: { header } })}
        />
      ) : null}
    </div>
  );
}

export function UpstreamEditorFields({
  draft,
  providerOptions,
  appProxyUrl,
  showApiKeys,
  onToggleApiKeys,
  onChangeDraft,
}: UpstreamEditorFieldsProps) {
  const providers = draft.providers.map((value) => value.trim()).filter(Boolean);
  const isKiro = providers.includes("kiro");
  const isCodex = providers.includes("codex");
  const isAccountBackedProvider = isKiro || isCodex;
  return (
    <div
      data-slot="upstream-editor-fields"
      className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-4"
    >
      <UpstreamIdentityFields
        draft={draft}
        providerOptions={providerOptions}
        appProxyUrl={appProxyUrl}
        showBaseUrl={!isAccountBackedProvider}
        showProxyUrl={!isAccountBackedProvider}
        onChangeDraft={onChangeDraft}
      />
      {isKiro || isCodex ? null : (
        <UpstreamAuthFields
          draft={draft}
          showApiKeys={showApiKeys}
          onToggleApiKeys={onToggleApiKeys}
          onChangeDraft={onChangeDraft}
        />
      )}
      <UpstreamOrderFields draft={draft} onChangeDraft={onChangeDraft} />
      <ConvertFromMapEditor
        key={draft.providers.join("|")}
        providers={draft.providers}
        value={draft.convertFromMap}
        onChange={(convertFromMap) => onChangeDraft({ convertFromMap })}
      />
      <UpstreamModelMappingFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamHeaderOverrideFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamOpenAIResponsesFields draft={draft} onChangeDraft={onChangeDraft} />
    </div>
  );
}
