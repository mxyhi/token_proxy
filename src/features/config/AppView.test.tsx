import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AppView } from "@/features/config/AppView";
import { EMPTY_FORM } from "@/features/config/form";
import type { ProxyServiceStatus } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

vi.mock("@/components/app-sidebar", () => ({
  AppSidebar: () => <div data-testid="app-sidebar" />,
}));

vi.mock("@/components/site-header", () => ({
  SiteHeader: ({ title }: { title: string }) => <div data-testid="site-header">{title}</div>,
}));

vi.mock("@/features/config/cards", () => ({
  ClientSetupCard: () => <div data-testid="client-setup-card" />,
  ConfigFileCard: () => <div data-testid="config-file-card" />,
  AutoStartCard: () => <div data-testid="auto-start-card" />,
  ProjectLinksCard: () => <div data-testid="project-links-card" />,
  ProxyCoreCard: () => <div data-testid="proxy-core-card" />,
  TrayTokenRateCard: () => <div data-testid="tray-token-rate-card" />,
  UpdateCard: () => <div data-testid="update-card" />,
  UpstreamsCard: () => <div data-testid="upstreams-card" />,
  ValidationCard: () => <div data-testid="validation-card" />,
}));

vi.mock("@/features/dashboard/DashboardPanel", () => ({
  DashboardPanel: () => <div data-testid="dashboard-panel" />,
}));

vi.mock("@/features/logs/LogsPanel", () => ({
  LogsPanel: () => <div data-testid="logs-panel" />,
}));

vi.mock("@/features/providers/ProvidersPanel", () => ({
  ProvidersPanel: () => <div data-testid="providers-panel" />,
}));

const IDLE_PROXY_STATUS: ProxyServiceStatus = {
  state: "stopped",
  addr: null,
  last_error: null,
};

describe("config/AppView", () => {
  it("does not show a persistent save button when there are pending edits", () => {
    render(
      <AppView
        activeSectionId="core"
        form={EMPTY_FORM}
        statusBadge={{ id: "saved", label: "saved", variant: "default" }}
        showLocalKey={false}
        showUpstreamKeys={false}
        providerOptions={[]}
        configPath="/tmp/config.json"
        savedAt=""
        autoStartEnabled={false}
        autoStartStatus="idle"
        autoStartMessage=""
        proxyServiceStatus={IDLE_PROXY_STATUS}
        proxyServiceRequestState="idle"
        proxyServiceMessage=""
        status="idle"
        statusMessage=""
        canSave
        isDirty
        validation={{ valid: true, message: "" }}
        onToggleLocalKey={() => undefined}
        onToggleUpstreamKeys={() => undefined}
        onFormChange={() => undefined}
        onStrategyChange={() => undefined}
        onAutoStartChange={() => undefined}
        onAddUpstream={() => undefined}
        onRemoveUpstream={() => undefined}
        onChangeUpstream={() => undefined}
        onReload={() => undefined}
        onSave={() => undefined}
        onProxyServiceRefresh={() => undefined}
        onProxyServiceStart={() => undefined}
        onProxyServiceStop={() => undefined}
        onProxyServiceRestart={() => undefined}
        onProxyServiceReload={() => undefined}
      />
    );

    expect(screen.getByRole("button", { name: m.common_refresh() })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: m.common_save() })).not.toBeInTheDocument();
  });

  it("shows retry action only inside error alert for dirty draft", () => {
    const onSave = vi.fn();

    render(
      <AppView
        activeSectionId="core"
        form={EMPTY_FORM}
        statusBadge={{ id: "saved", label: "saved", variant: "default" }}
        showLocalKey={false}
        showUpstreamKeys={false}
        providerOptions={[]}
        configPath="/tmp/config.json"
        savedAt=""
        autoStartEnabled={false}
        autoStartStatus="idle"
        autoStartMessage=""
        proxyServiceStatus={IDLE_PROXY_STATUS}
        proxyServiceRequestState="idle"
        proxyServiceMessage=""
        status="error"
        statusMessage="disk full"
        canSave
        isDirty
        validation={{ valid: true, message: "" }}
        onToggleLocalKey={() => undefined}
        onToggleUpstreamKeys={() => undefined}
        onFormChange={() => undefined}
        onStrategyChange={() => undefined}
        onAutoStartChange={() => undefined}
        onAddUpstream={() => undefined}
        onRemoveUpstream={() => undefined}
        onChangeUpstream={() => undefined}
        onReload={() => undefined}
        onSave={onSave}
        onProxyServiceRefresh={() => undefined}
        onProxyServiceStart={() => undefined}
        onProxyServiceStop={() => undefined}
        onProxyServiceRestart={() => undefined}
        onProxyServiceReload={() => undefined}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: m.config_retry_save() }));

    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it("does not render informational save alerts", () => {
    render(
      <AppView
        activeSectionId="core"
        form={EMPTY_FORM}
        statusBadge={{ id: "saved", label: "saved", variant: "default" }}
        showLocalKey={false}
        showUpstreamKeys={false}
        providerOptions={[]}
        configPath="/tmp/config.json"
        savedAt=""
        autoStartEnabled={false}
        autoStartStatus="idle"
        autoStartMessage=""
        proxyServiceStatus={IDLE_PROXY_STATUS}
        proxyServiceRequestState="idle"
        proxyServiceMessage=""
        status="saved"
        statusMessage="should not be shown"
        canSave={false}
        isDirty={false}
        validation={{ valid: true, message: "" }}
        onToggleLocalKey={() => undefined}
        onToggleUpstreamKeys={() => undefined}
        onFormChange={() => undefined}
        onStrategyChange={() => undefined}
        onAutoStartChange={() => undefined}
        onAddUpstream={() => undefined}
        onRemoveUpstream={() => undefined}
        onChangeUpstream={() => undefined}
        onReload={() => undefined}
        onSave={() => undefined}
        onProxyServiceRefresh={() => undefined}
        onProxyServiceStart={() => undefined}
        onProxyServiceStop={() => undefined}
        onProxyServiceRestart={() => undefined}
        onProxyServiceReload={() => undefined}
      />
    );

    expect(screen.queryByText("should not be shown")).not.toBeInTheDocument();
  });
});
