import { AlertCircle, Loader2, RefreshCw } from "lucide-react";
import { useMemo, type CSSProperties } from "react";

import { AppSidebar } from "@/components/app-sidebar";
import { SiteHeader } from "@/components/site-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import {
  ConfigFileCard,
  ProxyCoreCard,
  StrategyCard,
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
import { DashboardPanel } from "@/features/dashboard/DashboardPanel";
import { m } from "@/paraglide/messages.js";

type AppViewProps = {
  activeSectionId: ConfigSectionId;
  form: ConfigForm;
  statusBadge: StatusBadge;
  showLocalKey: boolean;
  showUpstreamKeys: boolean;
  providerOptions: string[];
  configPath: string;
  savedAt: string;
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
  onAddUpstream: (upstream: ConfigForm["upstreams"][number]) => void;
  onRemoveUpstream: (index: number) => void;
  onChangeUpstream: (
    index: number,
    patch: Partial<ConfigForm["upstreams"][number]>
  ) => void;
  onSave: () => void;
  onReset: () => void;
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
};

function ConfigToolbar({
  section,
  status,
  canSave,
  isDirty,
  onReload,
  onSave,
}: ConfigToolbarProps) {
  const isLoading = status === "loading";
  const isSaving = status === "saving";
  const canReload = !isLoading && !isDirty;

  return (
    <div
      data-slot="config-toolbar"
      className="sticky top-0 z-20 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border/60 bg-background/70 px-4 py-3"
    >
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-foreground">
          {section.label()}
        </p>
        <p className="truncate text-xs text-muted-foreground">
          {section.description()}
        </p>
      </div>
      <div className="flex items-center gap-2">
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
        <Button type="button" onClick={onSave} disabled={!canSave}>
          {isSaving ? (
            <Loader2 className="animate-spin" aria-hidden="true" />
          ) : (
            m.common_save()
          )}
        </Button>
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

type ConfigSectionBodyProps = ConfigSectionContentProps;

function ConfigSectionBody({
  activeSectionId,
  proxyService,
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
    case "strategy":
      return (
        <StrategyCard
          strategy={props.form.upstreamStrategy}
          onChange={props.onStrategyChange}
        />
      );
    case "upstreams":
      return (
        <UpstreamsCard
          upstreams={props.form.upstreams}
          showApiKeys={props.showUpstreamKeys}
          providerOptions={props.providerOptions}
          onToggleApiKeys={props.onToggleUpstreamKeys}
          onAdd={props.onAddUpstream}
          onRemove={props.onRemoveUpstream}
          onChange={props.onChangeUpstream}
        />
      );
    case "file":
      return (
        <div className="flex flex-col gap-4">
          <ConfigFileCard
            configPath={props.configPath}
            savedAt={props.savedAt}
            isDirty={props.isDirty}
            onReset={props.onReset}
          />
          <UpdateCard />
        </div>
      );
    case "validation":
      return <ValidationCard form={props.form} validation={props.validation} />;
    default:
      return null;
  }
}

function ConfigSectionContent({
  activeSectionId,
  proxyService,
  ...props
}: ConfigSectionContentProps) {
  if (activeSectionId === "dashboard") {
    return <DashboardPanel />;
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
      />
      <StatusAlert statusMessage={props.statusMessage} />
      <ConfigSectionBody
        {...props}
        activeSectionId={activeSectionId}
        proxyService={proxyService}
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
