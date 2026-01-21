import type { ReactNode } from "react";
import { CircleX, HelpCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type {
  HeaderOverrideForm,
  ModelMappingForm,
  UpstreamForm,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type EditorFieldProps = {
  label: string;
  htmlFor?: string;
  /** tooltip 提示文字，不传则不显示图标 */
  tooltip?: string;
  children: ReactNode;
};

/** 单个字段：label 左侧（可带 tooltip），input 右侧 */
export function EditorField({ label, htmlFor, tooltip, children }: EditorFieldProps) {
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

export function ModelMappingsEditor({ mappings, onChange }: ModelMappingsEditorProps) {
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

export function HeaderOverridesEditor({ overrides, onChange }: HeaderOverridesEditorProps) {
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
