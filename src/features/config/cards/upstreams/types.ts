import type { UpstreamForm } from "@/features/config/types";

export type UpstreamColumnId =
  | "id"
  | "provider"
  | "baseUrl"
  | "apiKeys"
  | "proxyUrl"
  | "priority"
  | "status";

export type UpstreamColumnDefinition = {
  id: UpstreamColumnId;
  label: () => string;
  defaultVisible: boolean;
  headerClassName?: string;
  cellClassName?: string;
};

export type ColumnVisibility = Record<UpstreamColumnId, boolean>;

/** autoId：ID 是否仍跟随 Base URL/provider 自动生成；用户手动输入后关闭，清空后恢复。 */
export type UpstreamEditorState =
  | { open: false }
  | { open: true; mode: "create"; draft: UpstreamForm; autoId: boolean }
  | { open: true; mode: "edit"; index: number; draft: UpstreamForm; autoId: boolean };

export type DeleteDialogState = { open: false } | { open: true; index: number };
