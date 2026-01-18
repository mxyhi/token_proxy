import { useEffect } from "react";

import { AppView } from "@/features/config/AppView";
import {
  useConfigDerived,
  useConfigState,
  useProxyServiceActions,
  useProxyServiceState,
} from "@/features/config/config-screen-state";
import { useConfigActions } from "@/features/config/config-screen-actions";
import { useConfigListActions } from "@/features/config/list-actions";
import type { ConfigSectionId } from "@/features/config/sections";
import type { ConfigForm } from "@/features/config/types";
import { useUpdater } from "@/features/update/updater";

type ConfigScreenProps = {
  activeSectionId: ConfigSectionId;
};

type ConfigState = ReturnType<typeof useConfigState>;
type ConfigDerived = ReturnType<typeof useConfigDerived>;
type ProxyServiceState = ReturnType<typeof useProxyServiceState>;
type ConfigListActions = ReturnType<typeof useConfigListActions>;
type ConfigActions = ReturnType<typeof useConfigActions>;
type ProxyServiceActions = ReturnType<typeof useProxyServiceActions>;

type AppViewArgs = {
  activeSectionId: ConfigSectionId;
  state: ConfigState;
  derived: ConfigDerived;
  proxyService: ProxyServiceState;
  listActions: ConfigListActions;
  configActions: ConfigActions;
  proxyActions: ProxyServiceActions;
};

function buildAppViewProps({
  activeSectionId,
  state,
  derived,
  proxyService,
  listActions,
  configActions,
  proxyActions,
}: AppViewArgs) {
  return {
    activeSectionId,
    form: state.form,
    statusBadge: derived.statusBadge,
    showLocalKey: state.showLocalKey,
    showUpstreamKeys: state.showUpstreamKeys,
    providerOptions: derived.providerOptions,
    configPath: state.configPath,
    savedAt: state.savedAt,
    autoStartEnabled: state.autoStartEnabled,
    autoStartStatus: state.autoStartStatus,
    autoStartMessage: state.autoStartMessage,
    proxyServiceStatus: proxyService.proxyServiceStatus,
    proxyServiceRequestState: proxyService.proxyServiceRequestState,
    proxyServiceMessage: proxyService.proxyServiceMessage,
    status: state.status,
    statusMessage: state.statusMessage,
    canSave: derived.canSave,
    isDirty: derived.isDirty,
    validation: derived.validation,
    onToggleLocalKey: () => state.setShowLocalKey((value) => !value),
    onToggleUpstreamKeys: () => state.setShowUpstreamKeys((value) => !value),
    onFormChange: state.updateForm,
    onStrategyChange: (value: ConfigForm["upstreamStrategy"]) =>
      state.updateForm({ upstreamStrategy: value }),
    onAutoStartChange: (value: boolean) => state.setAutoStartEnabled(value),
    onAddUpstream: listActions.addUpstream,
    onRemoveUpstream: listActions.removeUpstream,
    onChangeUpstream: listActions.updateUpstream,
    onSave: configActions.saveConfig,
    onReload: configActions.loadConfig,
    onProxyServiceRefresh: proxyActions.refreshProxyStatus,
    onProxyServiceStart: proxyActions.startProxy,
    onProxyServiceStop: proxyActions.stopProxy,
    onProxyServiceRestart: proxyActions.restartProxy,
    onProxyServiceReload: proxyActions.reloadProxy,
  };
}

export function ConfigScreen({ activeSectionId }: ConfigScreenProps) {
  const state = useConfigState();
  const derived = useConfigDerived(
    state.form,
    state.lastConfig,
    state.configExtras,
    state.status,
    state.autoStartEnabled,
    state.autoStartBaseline,
    state.autoStartStatus
  );
  const proxyService = useProxyServiceState();
  const proxyActions = useProxyServiceActions({
    setProxyServiceStatus: proxyService.setProxyServiceStatus,
    setProxyServiceRequestState: proxyService.setProxyServiceRequestState,
    setProxyServiceMessage: proxyService.setProxyServiceMessage,
  });
  const { refreshProxyStatus } = proxyActions;
  const configActions = useConfigActions({
    currentPayload: derived.currentPayload,
    validation: derived.validation,
    configDirty: derived.configDirty,
    autoStartEnabled: state.autoStartEnabled,
    autoStartBaseline: state.autoStartBaseline,
    autoStartStatus: state.autoStartStatus,
    setConfigPath: state.setConfigPath,
    setForm: state.setForm,
    setLastConfig: state.setLastConfig,
    setConfigExtras: state.setConfigExtras,
    setSavedAt: state.setSavedAt,
    setStatus: state.setStatus,
    setStatusMessage: state.setStatusMessage,
    setAutoStartEnabled: state.setAutoStartEnabled,
    setAutoStartBaseline: state.setAutoStartBaseline,
    setAutoStartStatus: state.setAutoStartStatus,
    setAutoStartMessage: state.setAutoStartMessage,
    setProxyServiceStatus: proxyService.setProxyServiceStatus,
    setProxyServiceMessage: proxyService.setProxyServiceMessage,
  });
  const { loadConfig } = configActions;
  const listActions = useConfigListActions(state.setForm);
  const {
    actions: { setAppProxyUrl },
  } = useUpdater();

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  useEffect(() => {
    if (!state.lastConfig) {
      return;
    }
    setAppProxyUrl(state.lastConfig.app_proxy_url ?? "");
  }, [setAppProxyUrl, state.lastConfig?.app_proxy_url]);

  useEffect(() => {
    void refreshProxyStatus();
  }, [refreshProxyStatus]);

  const appViewProps = buildAppViewProps({
    activeSectionId,
    state,
    derived,
    proxyService,
    listActions,
    configActions,
    proxyActions,
  });

  return <AppView {...appViewProps} />;
}
