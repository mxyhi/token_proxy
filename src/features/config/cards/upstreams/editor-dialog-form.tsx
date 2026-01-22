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
import { AntigravityAccountSelect } from "@/features/config/cards/upstreams/antigravity-account-select";
import { CodexAccountSelect } from "@/features/config/cards/upstreams/codex-account-select";
import {
  EditorField,
  HeaderOverridesEditor,
  ModelMappingsEditor,
} from "@/features/config/cards/upstreams/editor-fields";
import { KiroAccountSelect } from "@/features/config/cards/upstreams/kiro-account-select";
import { createModelMapping } from "@/features/config/form";
import type { AntigravityAccountSummary } from "@/features/antigravity/types";
import type { CodexAccountSummary } from "@/features/codex/types";
import type { KiroAccountSummary } from "@/features/kiro/types";
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
  kiroAccounts: KiroAccountSummary[];
  kiroAccountsLoading: boolean;
  kiroAccountsError: string;
  onRefreshKiroAccounts: () => void;
  codexAccounts: CodexAccountSummary[];
  codexAccountsLoading: boolean;
  codexAccountsError: string;
  onRefreshCodexAccounts: () => void;
  antigravityAccounts: AntigravityAccountSummary[];
  antigravityAccountsLoading: boolean;
  antigravityAccountsError: string;
  onRefreshAntigravityAccounts: () => void;
};

type UpstreamIdentityFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  appProxyUrl: string;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
  showBaseUrl: boolean;
  /** kiro 账户相关 props，仅 provider=kiro 时使用 */
  kiroAccounts: KiroAccountSummary[];
  kiroAccountsLoading: boolean;
  kiroAccountsError: string;
  onRefreshKiroAccounts: () => void;
  /** codex 账户相关 props，仅 provider=codex 时使用 */
  codexAccounts: CodexAccountSummary[];
  codexAccountsLoading: boolean;
  codexAccountsError: string;
  onRefreshCodexAccounts: () => void;
  /** antigravity 账户相关 props，仅 provider=antigravity 时使用 */
  antigravityAccounts: AntigravityAccountSummary[];
  antigravityAccountsLoading: boolean;
  antigravityAccountsError: string;
  onRefreshAntigravityAccounts: () => void;
};

function UpstreamIdentityFields({
  draft,
  providerOptions,
  appProxyUrl,
  onChangeDraft,
  showBaseUrl,
  kiroAccounts,
  kiroAccountsLoading,
  kiroAccountsError,
  onRefreshKiroAccounts,
  codexAccounts,
  codexAccountsLoading,
  codexAccountsError,
  onRefreshCodexAccounts,
  antigravityAccounts,
  antigravityAccountsLoading,
  antigravityAccountsError,
  onRefreshAntigravityAccounts,
}: UpstreamIdentityFieldsProps) {
  const canUseAppProxy = !!appProxyUrl.trim();
  const isKiro = draft.provider.trim() === "kiro";
  const isCodex = draft.provider.trim() === "codex";
  const isAntigravity = draft.provider.trim() === "antigravity";
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
        <Select
          value={draft.provider.trim() ? draft.provider : undefined}
          onValueChange={(value) => onChangeDraft({ provider: value })}
        >
          <SelectTrigger>
            <SelectValue placeholder="openai" />
          </SelectTrigger>
          <SelectContent>
            {providerOptions.map((option) => (
              <SelectItem key={option} value={option}>
                {option}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
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

      {/* 第二行：kiro / codex 账户选择器 */}
      {isKiro ? (
        <KiroAccountSelect
          accountId={draft.kiroAccountId}
          accounts={kiroAccounts}
          loading={kiroAccountsLoading}
          error={kiroAccountsError}
          onRefresh={onRefreshKiroAccounts}
          onSelect={(accountId) => onChangeDraft({ kiroAccountId: accountId })}
        />
      ) : null}
      {isCodex ? (
        <CodexAccountSelect
          accountId={draft.codexAccountId}
          accounts={codexAccounts}
          loading={codexAccountsLoading}
          error={codexAccountsError}
          onRefresh={onRefreshCodexAccounts}
          onSelect={(accountId) => onChangeDraft({ codexAccountId: accountId })}
        />
      ) : null}
      {isAntigravity ? (
        <AntigravityAccountSelect
          accountId={draft.antigravityAccountId}
          accounts={antigravityAccounts}
          loading={antigravityAccountsLoading}
          error={antigravityAccountsError}
          onRefresh={onRefreshAntigravityAccounts}
          onSelect={(accountId) => onChangeDraft({ antigravityAccountId: accountId })}
        />
      ) : null}
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
        htmlFor="upstream-editor-apiKey"
      >
        <PasswordInput
          id="upstream-editor-apiKey"
          visible={showApiKeys}
          onVisibilityChange={onToggleApiKeys}
          value={draft.apiKey}
          onChange={(e) => onChangeDraft({ apiKey: e.target.value })}
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
  if (draft.provider.trim() !== "openai-response") {
    return null;
  }

  return (
    <div data-slot="upstream-openai-responses-fields" className="contents">
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
  kiroAccounts,
  kiroAccountsLoading,
  kiroAccountsError,
  onRefreshKiroAccounts,
  codexAccounts,
  codexAccountsLoading,
  codexAccountsError,
  onRefreshCodexAccounts,
  antigravityAccounts,
  antigravityAccountsLoading,
  antigravityAccountsError,
  onRefreshAntigravityAccounts,
}: UpstreamEditorFieldsProps) {
  const isKiro = draft.provider.trim() === "kiro";
  const isCodex = draft.provider.trim() === "codex";
  const isAntigravity = draft.provider.trim() === "antigravity";
  return (
    <div
      data-slot="upstream-editor-fields"
      className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-4"
    >
      <UpstreamIdentityFields
        draft={draft}
        providerOptions={providerOptions}
        appProxyUrl={appProxyUrl}
        showBaseUrl={!isKiro}
        onChangeDraft={onChangeDraft}
        kiroAccounts={kiroAccounts}
        kiroAccountsLoading={kiroAccountsLoading}
        kiroAccountsError={kiroAccountsError}
        onRefreshKiroAccounts={onRefreshKiroAccounts}
        codexAccounts={codexAccounts}
        codexAccountsLoading={codexAccountsLoading}
        codexAccountsError={codexAccountsError}
        onRefreshCodexAccounts={onRefreshCodexAccounts}
        antigravityAccounts={antigravityAccounts}
        antigravityAccountsLoading={antigravityAccountsLoading}
        antigravityAccountsError={antigravityAccountsError}
        onRefreshAntigravityAccounts={onRefreshAntigravityAccounts}
      />
      {isKiro || isCodex || isAntigravity ? null : (
        <UpstreamAuthFields
          draft={draft}
          showApiKeys={showApiKeys}
          onToggleApiKeys={onToggleApiKeys}
          onChangeDraft={onChangeDraft}
        />
      )}
      <UpstreamOrderFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamModelMappingFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamHeaderOverrideFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamOpenAIResponsesFields draft={draft} onChangeDraft={onChangeDraft} />
    </div>
  );
}
