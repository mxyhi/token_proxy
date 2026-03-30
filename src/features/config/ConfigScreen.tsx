import { useEffect, useRef } from "react";

import { AppView } from "@/features/config/AppView";
import {
  useConfigDerived,
  useConfigState,
  useProxyServiceActions,
  useProxyServiceState,
} from "@/features/config/config-screen-state";
import { useConfigActions } from "@/features/config/config-screen-actions";
import { syncAccountBackedUpstreams } from "@/features/config/form";
import { useConfigListActions } from "@/features/config/list-actions";
import type { ConfigSectionId } from "@/features/config/sections";
import type { ConfigForm } from "@/features/config/types";
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
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
const CONFIG_AUTO_SAVE_DELAY_MS = 800;

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
  const lastObservedAutoSaveKeyRef = useRef("");
  const lastAttemptedAutoSaveKeyRef = useRef("");
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
  const { loadConfig, saveConfig } = configActions;
  const listActions = useConfigListActions(state.setForm);
  const kiroAccounts = useKiroAccounts();
  const codexAccounts = useCodexAccounts();
  const {
    actions: { setAppProxyUrl },
  } = useUpdater();
  const appProxyUrl = state.lastConfig?.app_proxy_url ?? "";

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  useEffect(() => {
    if (!state.lastConfig) {
      return;
    }
    setAppProxyUrl(appProxyUrl);
  }, [appProxyUrl, setAppProxyUrl, state.lastConfig]);

  useEffect(() => {
    void refreshProxyStatus();
  }, [refreshProxyStatus]);

  useEffect(() => {
    if (kiroAccounts.loading || codexAccounts.loading) {
      return;
    }
    state.setForm((prev) => {
      const nextUpstreams = syncAccountBackedUpstreams(prev.upstreams, {
        hasKiroAccount: kiroAccounts.accounts.length > 0,
        hasCodexAccount: codexAccounts.accounts.length > 0,
      });
      if (nextUpstreams === prev.upstreams) {
        return prev;
      }
      return {
        ...prev,
        upstreams: nextUpstreams,
      };
    });
  }, [
    codexAccounts.accounts,
    codexAccounts.loading,
    kiroAccounts.accounts,
    kiroAccounts.loading,
    state.setForm,
  ]);

  useEffect(() => {
    if (derived.autoSaveKey === lastObservedAutoSaveKeyRef.current) {
      return;
    }
    lastObservedAutoSaveKeyRef.current = derived.autoSaveKey;
    lastAttemptedAutoSaveKeyRef.current = "";
  }, [derived.autoSaveKey]);

  useEffect(() => {
    if (!derived.canAutoSave || !derived.autoSaveKey) {
      return;
    }
    if (derived.autoSaveKey === lastAttemptedAutoSaveKeyRef.current) {
      return;
    }
    const timerId = window.setTimeout(() => {
      // 失败后不应对同一份草稿无限重试；只有用户继续编辑形成新草稿时，才重新进入自动保存。
      lastAttemptedAutoSaveKeyRef.current = derived.autoSaveKey;
      void saveConfig();
    }, CONFIG_AUTO_SAVE_DELAY_MS);
    return () => window.clearTimeout(timerId);
  }, [derived.autoSaveKey, derived.canAutoSave, saveConfig]);

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
