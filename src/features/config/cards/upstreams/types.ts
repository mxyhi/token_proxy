import type { UpstreamForm } from "@/features/config/types";

export type UpstreamColumnId = "id" | "provider" | "baseUrl" | "apiKey" | "priority" | "status";

export type UpstreamColumnDefinition = {
  id: UpstreamColumnId;
  label: () => string;
  defaultVisible: boolean;
  headerClassName?: string;
  cellClassName?: string;
};

export type ColumnVisibility = Record<UpstreamColumnId, boolean>;

export type UpstreamEditorState =
  | { open: false }
  | { open: true; mode: "create"; draft: UpstreamForm }
  | { open: true; mode: "edit"; index: number; draft: UpstreamForm };

export type DeleteDialogState = { open: false } | { open: true; index: number };
