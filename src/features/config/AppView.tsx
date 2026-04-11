import { AlertCircle, RefreshCw } from "lucide-react";
import { useMemo } from "react";

import { AppShell } from "@/layouts/app-shell";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import {
  ClientSetupCard,
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
import { DashboardPanel } from "@/features/dashboard/DashboardPanel";
import { LogsPanel } from "@/features/logs/LogsPanel";
import { ProvidersPanel } from "@/features/providers/ProvidersPanel";
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
  isDirty: boolean;
  onReload: () => void;
};

function ConfigToolbar({
  section,
  status,
  isDirty,
  onReload,
}: ConfigToolbarProps) {
  const isLoading = status === "loading";
  const isSaving = status === "saving";
  const canReload = !isSaving && !isLoading;

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
      </div>
    </div>
  );
}

type StatusAlertProps = {
  status: AppViewProps["status"];
  statusMessage: string;
  canSave: boolean;
  isDirty: boolean;
  onSave: () => void;
};

type ValidationAlertProps = {
  validation: AppViewProps["validation"];
};

function ValidationAlert({ validation }: ValidationAlertProps) {
  if (validation.valid) {
    return null;
  }

  const message = validation.message || m.config_invalid_configuration();

  return (
    <Alert variant="destructive" className="mb-4">
      <AlertCircle className="size-4" aria-hidden="true" />
      <div className="flex flex-1 items-start gap-3">
        <div>
          <AlertTitle>{m.config_invalid_configuration()}</AlertTitle>
          <AlertDescription>{message}</AlertDescription>
        </div>
      </div>
    </Alert>
  );
}

function StatusAlert({
  status,
  statusMessage,
  canSave,
  isDirty,
  onSave,
}: StatusAlertProps) {
  if (status !== "error" || !statusMessage) {
    return null;
  }

  const canRetrySave = isDirty && canSave;

  return (
    <Alert variant="destructive" className="mb-4">
      <AlertCircle className="size-4" aria-hidden="true" />
      <div className="flex flex-1 items-start justify-between gap-3">
        <div>
          <AlertTitle>{m.config_request_failed_title()}</AlertTitle>
          <AlertDescription>{statusMessage}</AlertDescription>
        </div>
        {canRetrySave ? (
          <Button type="button" variant="outline" size="sm" onClick={onSave}>
            {m.config_retry_save()}
          </Button>
        ) : null}
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
          <ClientSetupCard savedAt={props.savedAt} isDirty={props.isDirty} />
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
        isDirty={props.isDirty}
        onReload={props.onReload}
      />
      <ValidationAlert validation={props.validation} />
      <StatusAlert
        status={props.status}
        statusMessage={props.statusMessage}
        canSave={props.canSave}
        isDirty={props.isDirty}
        onSave={props.onSave}
      />
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
    <AppShell title={sectionMeta.label()}>
      <ConfigSectionContent
        {...viewProps}
        activeSectionId={activeSectionId}
        proxyService={proxyService}
      />
    </AppShell>
  );
}
