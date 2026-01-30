import { AlertCircle, Loader2, Plus, RefreshCw } from "lucide-react";
import { useCallback, useMemo, useState, type CSSProperties } from "react";

import { AppSidebar } from "@/components/app-sidebar";
import { SiteHeader } from "@/components/site-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import {
  ConfigFileCard,
  AutoStartCard,
  ProjectLinksCard,
  ProxyCoreCard,
  TrayTokenRateCard,
  UpdateCard,
  UpstreamsCard,
  ValidationCard,
  type StatusBadge,
} from "@/features/config/cards";
import type { ProxyServiceViewProps } from "@/features/config/cards/proxy-service-card";
import type {
  ConfigSection,
  ConfigSectionId,
} from "@/features/config/sections";
import { findSection } from "@/features/config/sections";
import type {
  ConfigForm,
  ProxyServiceRequestState,
  ProxyServiceStatus,
} from "@/features/config/types";
import { AgentsPanel } from "@/features/agents";
import { DashboardPanel } from "@/features/dashboard/DashboardPanel";
import { LogsPanel } from "@/features/logs/LogsPanel";
import { ProvidersPanel } from "@/features/providers/ProvidersPanel";
import { m } from "@/paraglide/messages.js";

// Agent 工具类型（用于 agents 页面工具筛选与创建）
export type AgentToolId = "claude" | "codex" | "opencode";

type AppViewProps = {
  activeSectionId: ConfigSectionId;
  form: ConfigForm;
  statusBadge: StatusBadge;
  showLocalKey: boolean;
  showUpstreamKeys: boolean;
  providerOptions: string[];
  configPath: string;
  savedAt: string;
  autoStartEnabled: boolean;
  autoStartStatus: "idle" | "loading" | "error";
  autoStartMessage: string;
  proxyServiceStatus: ProxyServiceStatus | null;
  proxyServiceRequestState: ProxyServiceRequestState;
  proxyServiceMessage: string;
  status: "idle" | "loading" | "saving" | "saved" | "error";
  statusMessage: string;
  canSave: boolean;
  isDirty: boolean;
  validation: { valid: boolean; message: string };
  onToggleLocalKey: () => void;
  onToggleUpstreamKeys: () => void;
  onFormChange: (patch: Partial<ConfigForm>) => void;
  onStrategyChange: (value: ConfigForm["upstreamStrategy"]) => void;
  onAutoStartChange: (value: boolean) => void;
  onAddUpstream: (upstream: ConfigForm["upstreams"][number]) => void;
  onRemoveUpstream: (index: number) => void;
  onChangeUpstream: (
    index: number,
    patch: Partial<ConfigForm["upstreams"][number]>
  ) => void;
  onSave: () => void;
  onReload: () => void;
  onProxyServiceRefresh: () => void;
  onProxyServiceStart: () => void;
  onProxyServiceStop: () => void;
  onProxyServiceRestart: () => void;
  onProxyServiceReload: () => void;
};

type ConfigToolbarProps = {
  section: ConfigSection;
  status: AppViewProps["status"];
  canSave: boolean;
  isDirty: boolean;
  onReload: () => void;
  onSave: () => void;
  // agents 页面专用
  selectedTool?: AgentToolId;
  onToolChange?: (tool: AgentToolId) => void;
  onAddAgent?: () => void;
};

function ConfigToolbar({
  section,
  status,
  canSave,
  isDirty,
  onReload,
  onSave,
  selectedTool,
  onToolChange,
  onAddAgent,
}: ConfigToolbarProps) {
  const isLoading = status === "loading";
  const isSaving = status === "saving";
  const canReload = !isLoading && !isSaving;
  const isAgentsSection = section.id === "agents";

  return (
    <div
      data-slot="config-toolbar"
      className="sticky top-0 z-20 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border/60 bg-background/70 px-4 py-3"
    >
      {/* agents 页面显示工具选择器，其他页面显示标题 */}
      {isAgentsSection && selectedTool && onToolChange ? (
        <Select value={selectedTool} onValueChange={(v) => onToolChange(v as AgentToolId)}>
          <SelectTrigger size="sm" className="w-[140px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="claude">Claude Code</SelectItem>
            <SelectItem value="opencode">OpenCode</SelectItem>
            <SelectItem value="codex">Codex</SelectItem>
          </SelectContent>
        </Select>
      ) : (
        <div className="min-w-0">
          <p className="truncate text-sm font-medium text-foreground">
            {section.label()}
          </p>
          <p className="truncate text-xs text-muted-foreground">
            {section.description()}
          </p>
        </div>
      )}
      <div className="flex items-center gap-2">
        {isDirty ? (
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button type="button" variant="outline" size="icon" disabled={!canReload}>
                <RefreshCw
                  className={isLoading ? "animate-spin" : undefined}
                  aria-hidden="true"
                />
                <span className="sr-only">{m.common_refresh()}</span>
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>{m.config_file_discard_title()}</AlertDialogTitle>
                <AlertDialogDescription>
                  {m.config_file_discard_description()}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
                <AlertDialogAction type="button" onClick={onReload}>
                  {m.common_refresh()}
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        ) : (
          <Button
            type="button"
            variant="outline"
            size="icon"
            onClick={onReload}
            disabled={!canReload}
          >
            <RefreshCw
              className={isLoading ? "animate-spin" : undefined}
              aria-hidden="true"
            />
            <span className="sr-only">{m.common_refresh()}</span>
          </Button>
        )}
        {/* agents 页面显示添加按钮，其他页面显示保存按钮 */}
        {isAgentsSection && onAddAgent ? (
          <Button type="button" size="icon" onClick={onAddAgent}>
            <Plus aria-hidden="true" />
            <span className="sr-only">{m.agents_add()}</span>
          </Button>
        ) : (
          <Button type="button" onClick={onSave} disabled={!canSave}>
            {isSaving ? (
              <Loader2 className="animate-spin" aria-hidden="true" />
            ) : (
              m.common_save()
            )}
          </Button>
        )}
      </div>
    </div>
  );
}

type StatusAlertProps = {
  statusMessage: string;
};

function StatusAlert({ statusMessage }: StatusAlertProps) {
  if (!statusMessage) {
    return null;
  }
  return (
    <Alert variant="destructive" className="mb-4">
      <AlertCircle className="size-4" aria-hidden="true" />
      <div>
        <AlertTitle>{m.config_request_failed_title()}</AlertTitle>
        <AlertDescription>{statusMessage}</AlertDescription>
      </div>
    </Alert>
  );
}

type ConfigSectionContentProps = Omit<AppViewProps, "activeSectionId"> & {
  activeSectionId: ConfigSectionId;
  proxyService: ProxyServiceViewProps;
};

type ConfigSectionBodyProps = ConfigSectionContentProps & {
  selectedTool: AgentToolId;
  agentEditorTrigger: number;
};

function ConfigSectionBody({
  activeSectionId,
  proxyService,
  selectedTool,
  agentEditorTrigger,
  ...props
}: ConfigSectionBodyProps) {
  switch (activeSectionId) {
    case "core":
      return (
        <ProxyCoreCard
          form={props.form}
          showLocalKey={props.showLocalKey}
          onToggleLocalKey={props.onToggleLocalKey}
          onChange={props.onFormChange}
          proxyService={proxyService}
        />
      );
    case "upstreams":
      return (
        <div className="flex flex-col gap-4">
          <UpstreamsCard
            upstreams={props.form.upstreams}
            appProxyUrl={props.form.appProxyUrl}
            strategy={props.form.upstreamStrategy}
            showApiKeys={props.showUpstreamKeys}
            providerOptions={props.providerOptions}
            onToggleApiKeys={props.onToggleUpstreamKeys}
            onStrategyChange={props.onStrategyChange}
            onAdd={props.onAddUpstream}
            onRemove={props.onRemoveUpstream}
            onChange={props.onChangeUpstream}
          />
        </div>
      );
    case "settings":
      return (
        <div className="flex flex-col gap-4">
          <ConfigFileCard
            configPath={props.configPath}
            savedAt={props.savedAt}
            isDirty={props.isDirty}
          />
          <AutoStartCard
            enabled={props.autoStartEnabled}
            status={props.autoStartStatus}
            message={props.autoStartMessage}
            onChange={props.onAutoStartChange}
          />
          <TrayTokenRateCard
            value={props.form.trayTokenRate}
            onChange={(nextValue) => props.onFormChange({ trayTokenRate: nextValue })}
          />
          <ValidationCard form={props.form} validation={props.validation} />
          <ProjectLinksCard />
          <UpdateCard />
        </div>
      );
    case "agents":
      return (
        <div className="flex flex-col gap-4">
          <AgentsPanel selectedTool={selectedTool} addTrigger={agentEditorTrigger} />
        </div>
      );
    default:
      return null;
  }
}

function ConfigSectionContent({
  activeSectionId,
  proxyService,
  ...props
}: ConfigSectionContentProps) {
  // agents 页面的工具选择状态
  const [selectedTool, setSelectedTool] = useState<AgentToolId>("claude");
  // agents 页面的添加对话框触发器
  const [agentEditorTrigger, setAgentEditorTrigger] = useState(0);
  const handleAddAgent = useCallback(() => {
    setAgentEditorTrigger((prev) => prev + 1);
  }, []);

  if (activeSectionId === "dashboard") {
    return <DashboardPanel />;
  }
  if (activeSectionId === "logs") {
    return <LogsPanel />;
  }
  if (activeSectionId === "providers") {
    return <ProvidersPanel />;
  }

  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <ConfigToolbar
        section={findSection(activeSectionId)}
        status={props.status}
        canSave={props.canSave}
        isDirty={props.isDirty}
        onReload={props.onReload}
        onSave={props.onSave}
        selectedTool={selectedTool}
        onToolChange={setSelectedTool}
        onAddAgent={handleAddAgent}
      />
      <StatusAlert statusMessage={props.statusMessage} />
      <ConfigSectionBody
        {...props}
        activeSectionId={activeSectionId}
        proxyService={proxyService}
        selectedTool={selectedTool}
        agentEditorTrigger={agentEditorTrigger}
      />
    </div>
  );
}

function toProxyServiceViewProps(props: AppViewProps) {
  return {
    status: props.proxyServiceStatus,
    requestState: props.proxyServiceRequestState,
    message: props.proxyServiceMessage,
    isDirty: props.isDirty,
    onRefresh: props.onProxyServiceRefresh,
    onStart: props.onProxyServiceStart,
    onStop: props.onProxyServiceStop,
    onRestart: props.onProxyServiceRestart,
    onReload: props.onProxyServiceReload,
  };
}

export function AppView(props: AppViewProps) {
  const { activeSectionId, ...viewProps } = props;
  const sectionMeta = useMemo(
    () => findSection(activeSectionId),
    [activeSectionId]
  );
  const proxyService = toProxyServiceViewProps(props);

  return (
    <SidebarProvider
      className="h-full"
      style={
        {
          "--sidebar-width": "calc(var(--spacing) * 48)",
          "--header-height": "calc(var(--spacing) * 12)",
        } as CSSProperties
      }
    >
      <AppSidebar />
      <SidebarInset className="min-h-0 md:m-0 md:ml-0 md:rounded-none md:shadow-none">
        <div className="flex flex-1 min-h-0 flex-col">
          <ScrollArea className="flex-1 min-h-0">
            <div className="@container/main flex flex-1 flex-col gap-1">
              <SiteHeader title={sectionMeta.label()} />
              <div className="flex flex-col gap-2.5 py-2.5 md:gap-3.5 md:py-3.5">
                <ConfigSectionContent
                  {...viewProps}
                  activeSectionId={activeSectionId}
                  proxyService={proxyService}
                />
              </div>
            </div>
          </ScrollArea>
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}
