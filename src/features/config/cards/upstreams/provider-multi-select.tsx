import { ChevronDown } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

type ProviderMultiSelectProps = {
  providerOptions: readonly string[];
  value: readonly string[];
  disabled?: boolean;
  onChange: (next: string[]) => void;
};

function normalizeProviders(values: readonly string[]) {
  const output: string[] = [];
  const seen = new Set<string>();
  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed) {
      continue;
    }
    if (seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    output.push(trimmed);
  }
  return output;
}

function isSpecialProvider(provider: string) {
  return provider === "kiro" || provider === "codex";
}

function orderProviders(values: readonly string[], providerOptions: readonly string[]) {
  const index = new Map<string, number>();
  providerOptions.forEach((value, idx) => index.set(value, idx));
  return [...values].sort((left, right) => {
    const leftIndex = index.get(left);
    const rightIndex = index.get(right);
    if (leftIndex !== undefined && rightIndex !== undefined) {
      return leftIndex - rightIndex;
    }
    if (leftIndex !== undefined) return -1;
    if (rightIndex !== undefined) return 1;
    return left.localeCompare(right);
  });
}

function toggleProvider(
  current: readonly string[],
  providerOptions: readonly string[],
  provider: string,
  checked: boolean,
) {
  const normalized = normalizeProviders(current);
  if (!checked) {
    const next = normalized.filter((value) => value !== provider);
    // provider 必选：禁止清空最后一个选项（否则会导致后续字段“不同步/消失”）
    return next.length ? next : normalized;
  }

  if (isSpecialProvider(provider)) {
    return [provider];
  }
  const specialSelected = normalized.find(isSpecialProvider);
  const next = specialSelected ? [provider] : [...normalized, provider];
  return orderProviders(next, providerOptions);
}

export function ProviderMultiSelect({
  providerOptions,
  value,
  disabled = false,
  onChange,
}: ProviderMultiSelectProps) {
  const selected = orderProviders(normalizeProviders(value), providerOptions);
  const label = selected.length ? selected.join(", ") : "openai";

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          className="w-full justify-between"
          data-slot="provider-multi-select"
          disabled={disabled}
        >
          <span className="truncate">{label}</span>
          <ChevronDown className="size-4 text-muted-foreground" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[var(--radix-dropdown-menu-trigger-width)]">
        {providerOptions.map((option) => {
          const checked = selected.includes(option);
          return (
            <DropdownMenuCheckboxItem
              key={option}
              checked={checked}
              onCheckedChange={(nextChecked) =>
                disabled
                  ? undefined
                  : onChange(toggleProvider(selected, providerOptions, option, nextChecked === true))
              }
            >
              {option}
            </DropdownMenuCheckboxItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
