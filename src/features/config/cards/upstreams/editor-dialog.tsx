import { CircleX } from "lucide-react";
import type { ReactNode } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
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
import { getUpstreamLabel } from "@/features/config/cards/upstreams/constants";
import type { UpstreamEditorState } from "@/features/config/cards/upstreams/types";
import { createModelMapping } from "@/features/config/form";
import type {
  HeaderOverrideForm,
  ModelMappingForm,
  UpstreamForm,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type EditorFieldProps = {
  label: string;
  htmlFor?: string;
  children: ReactNode;
};

/** 单个字段：label 左侧，input 右侧 */
function EditorField({ label, htmlFor, children }: EditorFieldProps) {
  return (
    <>
      {htmlFor ? (
        <Label htmlFor={htmlFor}>{label}</Label>
      ) : (
        <Label>{label}</Label>
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
  const handleAdd = () => {
    onChange([...mappings, createModelMapping()]);
  };

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
      {mappings.length ? (
        <>
          <div className="grid grid-cols-[1fr_1fr_auto] gap-2 text-xs text-muted-foreground">
            <span>{m.field_model_mapping_pattern()}</span>
            <span>{m.field_model_mapping_target()}</span>
            <span className="sr-only">{m.model_mappings_remove()}</span>
          </div>
          {mappings.map((mapping, index) => (
            <div
              key={mapping.id}
              className="grid grid-cols-[1fr_1fr_auto] gap-2"
            >
              <Input
                value={mapping.pattern}
                onChange={(e) =>
                  handleUpdate(index, { pattern: e.target.value })
                }
                placeholder={m.model_mappings_placeholder_pattern()}
              />
              <Input
                value={mapping.target}
                onChange={(e) =>
                  handleUpdate(index, { target: e.target.value })
                }
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
        </>
      ) : (
        <p className="text-xs text-muted-foreground">
          {m.model_mappings_empty()}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="secondary" size="sm" onClick={handleAdd}>
          {m.model_mappings_add()}
        </Button>
        <span className="text-xs text-muted-foreground">
          {m.model_mappings_tip()}
        </span>
      </div>
    </div>
  );
}

type HeaderOverridesEditorProps = {
  overrides: UpstreamForm["overrides"]["header"];
  onChange: (next: UpstreamForm["overrides"]["header"]) => void;
};

function HeaderOverridesEditor({ overrides, onChange }: HeaderOverridesEditorProps) {
  const handleAdd = () => {
    const next: HeaderOverrideForm = {
      id: `header-override-${Date.now()}-${overrides.length}`,
      name: "",
      value: "",
      isNull: false,
    };
    onChange([...overrides, next]);
  };

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
      {overrides.length ? (
        <>
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
        </>
      ) : (
        <p className="text-xs text-muted-foreground">{m.header_overrides_empty()}</p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="secondary" size="sm" onClick={handleAdd}>
          {m.header_overrides_add()}
        </Button>
        <span className="text-xs text-muted-foreground">{m.header_overrides_tip()}</span>
      </div>
    </div>
  );
}

type UpstreamEditorFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

type UpstreamIdentityFieldsProps = {
  draft: UpstreamForm;
  providerOptions: readonly string[];
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function UpstreamIdentityFields({
  draft,
  providerOptions,
  onChangeDraft,
}: UpstreamIdentityFieldsProps) {
  return (
    <div data-slot="upstream-identity-fields" className="contents">
      <EditorField label={m.field_id()} htmlFor="upstream-editor-id">
        <Input
          id="upstream-editor-id"
          value={draft.id}
          onChange={(e) => onChangeDraft({ id: e.target.value })}
          placeholder="openai-default"
        />
      </EditorField>

      {/* Provider 和 Status 并排：四列布局 */}
      <Label>{m.field_provider()}</Label>
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
        <Label>{m.field_status()}</Label>
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

      <EditorField label={m.field_base_url()} htmlFor="upstream-editor-baseUrl">
        <Input
          id="upstream-editor-baseUrl"
          value={draft.baseUrl}
          onChange={(e) => onChangeDraft({ baseUrl: e.target.value })}
          placeholder="https://api.openai.com"
        />
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
      <EditorField label={m.field_api_key()} htmlFor="upstream-editor-apiKey">
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
      {/* Priority 和 Index 并排：四列布局 */}
      <Label htmlFor="upstream-editor-priority">{m.field_priority()}</Label>
      <div className="grid grid-cols-[1fr_auto_1fr] items-center gap-3">
        <Input
          id="upstream-editor-priority"
          value={draft.priority}
          onChange={(e) => onChangeDraft({ priority: e.target.value })}
          placeholder="0"
          inputMode="numeric"
        />
        <Label htmlFor="upstream-editor-index">{m.field_index()}</Label>
        <Input
          id="upstream-editor-index"
          value={draft.index}
          onChange={(e) => onChangeDraft({ index: e.target.value })}
          placeholder={m.common_optional()}
          inputMode="numeric"
        />
      </div>
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
  return (
    <div data-slot="upstream-model-mapping-fields" className="contents">
      <Label className="self-start">{m.field_model_mappings()}</Label>
      <ModelMappingsEditor
        mappings={draft.modelMappings}
        onChange={(next) => onChangeDraft({ modelMappings: next })}
      />
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
  return (
    <div data-slot="upstream-header-override-fields" className="contents">
      <Label className="self-start">{m.field_header_overrides()}</Label>
      <HeaderOverridesEditor
        overrides={draft.overrides.header}
        onChange={(header) => onChangeDraft({ overrides: { header } })}
      />
    </div>
  );
}

function UpstreamEditorFields({
  draft,
  providerOptions,
  showApiKeys,
  onToggleApiKeys,
  onChangeDraft,
}: UpstreamEditorFieldsProps) {
  return (
    <div
      data-slot="upstream-editor-fields"
      className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-4"
    >
      <UpstreamIdentityFields
        draft={draft}
        providerOptions={providerOptions}
        onChangeDraft={onChangeDraft}
      />
      <UpstreamAuthFields
        draft={draft}
        showApiKeys={showApiKeys}
        onToggleApiKeys={onToggleApiKeys}
        onChangeDraft={onChangeDraft}
      />
      <UpstreamOrderFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamModelMappingFields draft={draft} onChangeDraft={onChangeDraft} />
      <UpstreamHeaderOverrideFields draft={draft} onChangeDraft={onChangeDraft} />
    </div>
  );
}

type UpstreamEditorDialogProps = {
  editor: UpstreamEditorState;
  providerOptions: readonly string[];
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onOpenChange: (open: boolean) => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
  onSave: () => void;
};

export function UpstreamEditorDialog({
  editor,
  providerOptions,
  showApiKeys,
  onToggleApiKeys,
  onOpenChange,
  onChangeDraft,
  onSave,
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
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogBody className="space-y-4 pr-1">
          {editor.open ? (
            <UpstreamEditorFields
              draft={editor.draft}
              providerOptions={providerOptions}
              showApiKeys={showApiKeys}
              onToggleApiKeys={onToggleApiKeys}
              onChangeDraft={onChangeDraft}
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
