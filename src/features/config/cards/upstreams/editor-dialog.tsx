import { CirclePlus, CircleX, HelpCircle } from "lucide-react";
import type { ReactNode } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogBody,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { getUpstreamLabel } from "@/features/config/cards/upstreams/constants";
import { KiroAccountSelect } from "@/features/config/cards/upstreams/kiro-account-select";
import type { UpstreamEditorState } from "@/features/config/cards/upstreams/types";
import { createModelMapping } from "@/features/config/form";
import type { KiroAccountSummary } from "@/features/kiro/types";
import type {
  HeaderOverrideForm,
  KiroPreferredEndpoint,
  ModelMappingForm,
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

type EditorFieldProps = {
  label: string;
  htmlFor?: string;
  /** tooltip 提示文字，不传则不显示图标 */
  tooltip?: string;
  children: ReactNode;
};

/** 单个字段：label 左侧（可带 tooltip），input 右侧 */
function EditorField({ label, htmlFor, tooltip, children }: EditorFieldProps) {
  const labelContent = (
    <span className="inline-flex items-center gap-1">
      {label}
      {tooltip ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
          </TooltipTrigger>
          <TooltipContent side="right" className="max-w-xs">
            {tooltip}
          </TooltipContent>
        </Tooltip>
      ) : null}
    </span>
  );

  return (
    <>
      {htmlFor ? (
        <Label htmlFor={htmlFor}>{labelContent}</Label>
      ) : (
        <Label>{labelContent}</Label>
      )}
      {children}
    </>
  );
}

type ModelMappingsEditorProps = {
  mappings: ModelMappingForm[];
  onChange: (next: ModelMappingForm[]) => void;
};

function ModelMappingsEditor({ mappings, onChange }: ModelMappingsEditorProps) {
  const handleUpdate = (index: number, patch: Partial<ModelMappingForm>) => {
    onChange(
      mappings.map((mapping, current) =>
        current === index ? { ...mapping, ...patch } : mapping
      )
    );
  };

  const handleRemove = (index: number) => {
    onChange(mappings.filter((_, current) => current !== index));
  };

  return (
    <div data-slot="model-mappings" className="space-y-2">
      <div className="grid grid-cols-[1fr_1fr_auto] gap-2 text-xs text-muted-foreground">
        <span>{m.field_model_mapping_pattern()}</span>
        <span>{m.field_model_mapping_target()}</span>
        <span className="sr-only">{m.model_mappings_remove()}</span>
      </div>
      {mappings.map((mapping, index) => (
        <div key={mapping.id} className="grid grid-cols-[1fr_1fr_auto] gap-2">
          <Input
            value={mapping.pattern}
            onChange={(e) => handleUpdate(index, { pattern: e.target.value })}
            placeholder={m.model_mappings_placeholder_pattern()}
          />
          <Input
            value={mapping.target}
            onChange={(e) => handleUpdate(index, { target: e.target.value })}
            placeholder={m.model_mappings_placeholder_target()}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={m.model_mappings_remove()}
            onClick={() => handleRemove(index)}
          >
            <CircleX className="size-4" aria-hidden="true" />
          </Button>
        </div>
      ))}
    </div>
  );
}

type HeaderOverridesEditorProps = {
  overrides: UpstreamForm["overrides"]["header"];
  onChange: (next: UpstreamForm["overrides"]["header"]) => void;
};

function HeaderOverridesEditor({ overrides, onChange }: HeaderOverridesEditorProps) {
  const handleUpdate = (index: number, patch: Partial<HeaderOverrideForm>) => {
    onChange(
      overrides.map((item, current) => (current === index ? { ...item, ...patch } : item)),
    );
  };

  const handleRemove = (index: number) => {
    onChange(overrides.filter((_, current) => current !== index));
  };

  return (
    <div data-slot="header-overrides" className="space-y-2">
      <div className="grid grid-cols-[1fr_1fr_auto_auto] gap-2 text-xs text-muted-foreground">
        <span>{m.header_overrides_placeholder_name()}</span>
        <span>{m.header_overrides_placeholder_value()}</span>
        <span className="sr-only">{m.common_enabled()}</span>
        <span className="sr-only">{m.header_overrides_remove()}</span>
      </div>
      {overrides.map((item, index) => (
        <div
          key={item.id}
          className="grid grid-cols-[1fr_1fr_auto_auto] items-center gap-2"
        >
          <Input
            value={item.name}
            onChange={(e) => handleUpdate(index, { name: e.target.value })}
            placeholder={m.header_overrides_placeholder_name()}
          />
          <Input
            value={item.isNull ? "" : item.value}
            onChange={(e) => handleUpdate(index, { value: e.target.value, isNull: false })}
            placeholder={m.header_overrides_placeholder_value()}
            disabled={item.isNull}
          />
          <Button
            type="button"
            variant={item.isNull ? "secondary" : "ghost"}
            size="icon-sm"
            aria-label={item.isNull ? m.common_disabled() : m.common_enabled()}
            onClick={() => handleUpdate(index, { isNull: !item.isNull })}
          >
            {item.isNull ? m.common_disabled() : m.common_enabled()}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={m.header_overrides_remove()}
            onClick={() => handleRemove(index)}
          >
            <CircleX className="size-4" aria-hidden="true" />
          </Button>
        </div>
      ))}
    </div>
  );
}

type UpstreamEditorFieldsProps = {
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
}: UpstreamIdentityFieldsProps) {
  const canUseAppProxy = !!appProxyUrl.trim();
  const isKiro = draft.provider.trim() === "kiro";
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

      {/* 第二行：kiro 账户选择器（仅 kiro provider） */}
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
        <EditorField label={m.field_base_url()} tooltip={m.field_base_url_tip()} htmlFor="upstream-editor-baseUrl">
          <Input
            id="upstream-editor-baseUrl"
            value={draft.baseUrl}
            onChange={(e) => onChangeDraft({ baseUrl: e.target.value })}
            placeholder="https://api.openai.com"
          />
        </EditorField>
      ) : null}

      <EditorField label={m.field_proxy_url()} tooltip={m.upstreams_proxy_tip({ placeholder: "$app_proxy_url" })} htmlFor="upstream-editor-proxyUrl">
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
      <EditorField label={m.field_api_key()} tooltip={m.field_api_key_tip()} htmlFor="upstream-editor-apiKey">
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

type UpstreamOrderFieldsProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamOrderFields({
  draft,
  onChangeDraft,
}: UpstreamOrderFieldsProps) {
  return (
    <div data-slot="upstream-order-fields" className="contents">
      <EditorField label={m.field_priority()} tooltip={m.field_priority_tip()} htmlFor="upstream-editor-priority">
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

function UpstreamEditorFields({
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
}: UpstreamEditorFieldsProps) {
  const isKiro = draft.provider.trim() === "kiro";
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
      />
      {isKiro ? null : (
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
    </div>
  );
}

type UpstreamEditorDialogProps = {
  editor: UpstreamEditorState;
  providerOptions: readonly string[];
  appProxyUrl: string;
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onOpenChange: (open: boolean) => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
  onSave: () => void;
  kiroAccounts: KiroAccountSummary[];
  kiroAccountsLoading: boolean;
  kiroAccountsError: string;
  onRefreshKiroAccounts: () => void;
};

export function UpstreamEditorDialog({
  editor,
  providerOptions,
  appProxyUrl,
  showApiKeys,
  onToggleApiKeys,
  onOpenChange,
  onChangeDraft,
  onSave,
  kiroAccounts,
  kiroAccountsLoading,
  kiroAccountsError,
  onRefreshKiroAccounts,
}: UpstreamEditorDialogProps) {
  const title = editor.open
    ? editor.mode === "create"
      ? m.upstreams_editor_title_add()
      : m.upstreams_editor_title_edit()
    : m.upstreams_editor_title_default();
  const description =
    editor.open && editor.mode === "edit"
      ? m.upstreams_editor_description_edit({
          rowLabel: getUpstreamLabel(editor.index),
        })
      : m.upstreams_editor_description_create();

  return (
    <AlertDialog open={editor.open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-w-xl">
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            {title}
            <Tooltip>
              <TooltipTrigger asChild>
                <HelpCircle className="size-4 text-muted-foreground cursor-help" />
              </TooltipTrigger>
              <TooltipContent side="right" className="max-w-xs">
                {description}
              </TooltipContent>
            </Tooltip>
          </AlertDialogTitle>
        </AlertDialogHeader>
        <AlertDialogBody className="space-y-4 pr-1">
          {editor.open ? (
            <UpstreamEditorFields
              draft={editor.draft}
              providerOptions={providerOptions}
              appProxyUrl={appProxyUrl}
              showApiKeys={showApiKeys}
              onToggleApiKeys={onToggleApiKeys}
              onChangeDraft={onChangeDraft}
          kiroAccounts={kiroAccounts}
          kiroAccountsLoading={kiroAccountsLoading}
          kiroAccountsError={kiroAccountsError}
          onRefreshKiroAccounts={onRefreshKiroAccounts}
        />
          ) : null}
        </AlertDialogBody>
        <AlertDialogFooter>
          <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
          <AlertDialogAction onClick={onSave}>
            {m.common_save()}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
