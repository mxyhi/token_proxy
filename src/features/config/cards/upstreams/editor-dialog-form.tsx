import { ChevronDown, CirclePlus, HelpCircle } from "lucide-react";
import type { ReactNode } from "react";

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
import { AccountCredentialPanel } from "@/features/config/cards/upstreams/account-credential-panel";
import { AvailableModelsEditor } from "@/features/config/cards/upstreams/available-models-editor";
import { ConvertFromMapEditor } from "@/features/config/cards/upstreams/convert-from-map-editor";
import {
  EditorField,
  HeaderOverridesEditor,
  ModelMappingsEditor,
} from "@/features/config/cards/upstreams/editor-fields";
import { ProviderMultiSelect } from "@/features/config/cards/upstreams/provider-multi-select";
import {
  isAccountBackedProviderSet,
  isAccountIdentityLocked,
  isAccountProviderKind,
} from "@/features/config/cards/upstreams/upstream-editor-helpers";
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

export type UpstreamEditorFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  appProxyUrl: string;
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

type EditorSectionProps = {
  title: string;
  description: string;
  children: ReactNode;
};

function EditorSection({ title, description, children }: EditorSectionProps) {
  return (
    <section className="space-y-4 border-b pb-5 last:border-b-0 last:pb-0">
      <div className="space-y-1">
        <h3 className="text-sm font-semibold">{title}</h3>
        <p className="text-xs text-muted-foreground">{description}</p>
      </div>
      {children}
    </section>
  );
}

type UpstreamConnectionFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamConnectionFields({
  draft,
  providerOptions,
  showApiKeys,
  onToggleApiKeys,
  onChangeDraft,
}: UpstreamConnectionFieldsProps) {
  const providers = draft.providers.map((value) => value.trim()).filter(Boolean);
  const isAccountBacked = isAccountBackedProviderSet(providers);
  const isKiro = providers.includes("kiro");
  const identityLocked = isAccountIdentityLocked(draft);
  // 编辑既有账户 Upstream 时，mergeProviderOptions 已排除 Kiro/Codex/xAI；
  // 将当前 draft.providers 并回选项，保证展示与锁定身份一致。
  const editorProviderOptions = (() => {
    const seen = new Set(providerOptions);
    const extras = providers.filter((provider) => !seen.has(provider));
    return extras.length ? [...providerOptions, ...extras] : providerOptions;
  })();
  const kiroEndpointValue = draft.preferredEndpoint.trim()
    ? draft.preferredEndpoint
    : KIRO_ENDPOINT_INHERIT;

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-[minmax(7rem,auto)_1fr] items-center gap-x-4 gap-y-4">
        <EditorField label={m.field_provider()} tooltip={m.field_provider_tip()}>
          <ProviderMultiSelect
            providerOptions={editorProviderOptions}
            value={draft.providers}
            disabled={identityLocked}
            onChange={(next) => onChangeDraft({ providers: next })}
          />
        </EditorField>

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

        {isAccountBacked ? null : (
          <>
            <EditorField
              label={m.field_base_url()}
              tooltip={m.field_base_url_tip()}
              htmlFor="upstream-editor-baseUrl"
            >
              <Input
                id="upstream-editor-baseUrl"
                value={draft.baseUrl}
                onChange={(event) => onChangeDraft({ baseUrl: event.target.value })}
                placeholder="https://api.openai.com"
              />
            </EditorField>
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
                onChange={(event) => onChangeDraft({ apiKeys: event.target.value })}
                placeholder={m.common_optional()}
              />
            </EditorField>
          </>
        )}
      </div>

      {providers.length === 1 && providers[0] && isAccountProviderKind(providers[0]) ? (
        <AccountCredentialPanel draft={draft} />
      ) : null}
    </div>
  );
}

type UpstreamOpenAIResponsesFieldsProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function CompatibilitySwitch({
  label,
  tooltip,
  ariaLabel,
  checked,
  onCheckedChange,
}: {
  label: string;
  tooltip: string;
  ariaLabel: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <div className="col-span-2 flex items-center justify-between gap-4 rounded-md border px-3 py-2">
      <Label className="inline-flex items-center gap-1 font-normal">
        {label}
        <Tooltip>
          <TooltipTrigger asChild>
            <HelpCircle className="size-3.5 cursor-help text-muted-foreground" />
          </TooltipTrigger>
          <TooltipContent side="right" className="max-w-xs">
            {tooltip}
          </TooltipContent>
        </Tooltip>
      </Label>
      <Switch checked={checked} onCheckedChange={onCheckedChange} aria-label={ariaLabel} />
    </div>
  );
}

function UpstreamOpenAIResponsesFields({
  draft,
  onChangeDraft,
}: UpstreamOpenAIResponsesFieldsProps) {
  const isOpenai = draft.providers.some((value) => value.trim() === "openai");
  const isOpenaiResponses = draft.providers.some(
    (value) => value.trim() === "openai-response",
  );
  if (!isOpenai && !isOpenaiResponses) {
    return null;
  }

  return (
    <div data-slot="upstream-compatibility-fields" className="contents">
      {isOpenaiResponses ? (
        <CompatibilitySwitch
          label={m.field_use_chat_completions_for_responses()}
          tooltip={m.field_use_chat_completions_for_responses_tip()}
          ariaLabel={m.field_use_chat_completions_for_responses_aria()}
          checked={draft.useChatCompletionsForResponses}
          onCheckedChange={(checked) =>
            onChangeDraft({ useChatCompletionsForResponses: checked })
          }
        />
      ) : null}
      {isOpenaiResponses ? (
        <>
          <CompatibilitySwitch
            label={m.field_filter_prompt_cache_retention()}
            tooltip={m.field_filter_prompt_cache_retention_tip()}
            ariaLabel={m.field_filter_prompt_cache_retention_aria()}
            checked={draft.filterPromptCacheRetention}
            onCheckedChange={(checked) =>
              onChangeDraft({ filterPromptCacheRetention: checked })
            }
          />
          <CompatibilitySwitch
            label={m.field_filter_safety_identifier()}
            tooltip={m.field_filter_safety_identifier_tip()}
            ariaLabel={m.field_filter_safety_identifier_aria()}
            checked={draft.filterSafetyIdentifier}
            onCheckedChange={(checked) =>
              onChangeDraft({ filterSafetyIdentifier: checked })
            }
          />
        </>
      ) : null}
      <CompatibilitySwitch
        label={m.field_rewrite_developer_role_to_system()}
        tooltip={m.field_rewrite_developer_role_to_system_tip()}
        ariaLabel={m.field_rewrite_developer_role_to_system_aria()}
        checked={draft.rewriteDeveloperRoleToSystem}
        onCheckedChange={(checked) =>
          onChangeDraft({ rewriteDeveloperRoleToSystem: checked })
        }
      />
    </div>
  );
}

function UpstreamModelMappingFields({
  draft,
  onChangeDraft,
}: UpstreamOpenAIResponsesFieldsProps) {
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
              <HelpCircle className="size-3.5 cursor-help text-muted-foreground" />
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
      {draft.modelMappings.length ? (
        <ModelMappingsEditor
          mappings={draft.modelMappings}
          onChange={(modelMappings) => onChangeDraft({ modelMappings })}
        />
      ) : null}
    </div>
  );
}

function UpstreamHeaderOverrideFields({
  draft,
  onChangeDraft,
}: UpstreamOpenAIResponsesFieldsProps) {
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
              <HelpCircle className="size-3.5 cursor-help text-muted-foreground" />
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
      {draft.overrides.header.length ? (
        <HeaderOverridesEditor
          overrides={draft.overrides.header}
          onChange={(header) => onChangeDraft({ overrides: { header } })}
        />
      ) : null}
    </div>
  );
}

type UpstreamAdvancedFieldsProps = UpstreamOpenAIResponsesFieldsProps & {
  appProxyUrl: string;
};

function UpstreamAdvancedFields({
  draft,
  appProxyUrl,
  onChangeDraft,
}: UpstreamAdvancedFieldsProps) {
  const identityLocked = isAccountIdentityLocked(draft);
  const canUseAppProxy = !!appProxyUrl.trim();

  return (
    <details className="group">
      <summary className="flex cursor-pointer list-none items-center justify-between gap-4 rounded-md py-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <div className="space-y-1">
          <h3 className="text-sm font-semibold">{m.upstreams_section_advanced()}</h3>
          <p className="text-xs text-muted-foreground">
            {m.upstreams_section_advanced_summary()}
          </p>
        </div>
        <ChevronDown
          className="size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180"
          aria-hidden="true"
        />
      </summary>
      <div className="mt-4 grid grid-cols-[minmax(7rem,auto)_1fr] items-center gap-x-4 gap-y-4 border-t pt-4">
        <EditorField label={m.field_id()} tooltip={m.field_id_tip()} htmlFor="upstream-editor-id">
          <Input
            id="upstream-editor-id"
            value={draft.id}
            disabled={identityLocked}
            onChange={(event) => onChangeDraft({ id: event.target.value })}
            placeholder="openai-default"
          />
        </EditorField>

        <EditorField
          label={m.field_proxy_url()}
          tooltip={m.upstreams_proxy_tip({ placeholder: "$app_proxy_url" })}
          htmlFor="upstream-editor-proxyUrl"
        >
          <div className="flex items-center gap-2">
            <Input
              id="upstream-editor-proxyUrl"
              value={draft.proxyUrl}
              onChange={(event) => onChangeDraft({ proxyUrl: event.target.value })}
              placeholder="http://127.0.0.1:7890"
              className="min-w-0 flex-1"
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

        <EditorField
          label={m.field_priority()}
          tooltip={m.field_priority_tip()}
          htmlFor="upstream-editor-priority"
        >
          <Input
            id="upstream-editor-priority"
            value={draft.priority}
            onChange={(event) => onChangeDraft({ priority: event.target.value })}
            placeholder="0"
            inputMode="numeric"
          />
        </EditorField>

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
    </details>
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
  return (
    <div data-slot="upstream-editor-fields" className="space-y-5">
      <EditorSection
        title={m.upstreams_section_connection()}
        description={m.upstreams_section_connection_desc()}
      >
        <UpstreamConnectionFields
          draft={draft}
          providerOptions={providerOptions}
          showApiKeys={showApiKeys}
          onToggleApiKeys={onToggleApiKeys}
          onChangeDraft={onChangeDraft}
        />
      </EditorSection>

      <EditorSection
        title={m.upstreams_section_models()}
        description={m.upstreams_section_models_desc()}
      >
        <AvailableModelsEditor
          key={draft.providers.join("|")}
          draft={draft}
          onChangeDraft={onChangeDraft}
        />
      </EditorSection>

      <UpstreamAdvancedFields
        draft={draft}
        appProxyUrl={appProxyUrl}
        onChangeDraft={onChangeDraft}
      />
    </div>
  );
}
