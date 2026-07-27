import { invoke } from "@tauri-apps/api/core";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  type ReactNode,
} from "react";

import { AppView } from "@/features/config/AppView";
import {
  useConfigDerived,
  useConfigState,
  useProxyServiceActions,
  useProxyServiceState,
} from "@/features/config/config-screen-state";
import { useConfigActions } from "@/features/config/config-screen-actions";
import { createModelMapping } from "@/features/config/form";
import { useConfigListActions } from "@/features/config/list-actions";
import type { ConfigEditorSectionId } from "@/features/config/sections";
import type { ConfigForm } from "@/features/config/types";
import { useUpdater } from "@/features/update/updater";
import { parseError } from "@/lib/error";

type ConfigScreenProps = {
  activeSectionId: ConfigEditorSectionId;
};

type ConfigScreenProviderProps = {
  children: ReactNode;
};

type ConfigState = ReturnType<typeof useConfigState>;
type ConfigDerived = ReturnType<typeof useConfigDerived>;
type ProxyServiceState = ReturnType<typeof useProxyServiceState>;
type ConfigListActions = ReturnType<typeof useConfigListActions>;
type ConfigActions = ReturnType<typeof useConfigActions>;
type ProxyServiceActions = ReturnType<typeof useProxyServiceActions>;
const CONFIG_AUTO_SAVE_DELAY_MS = 800;

type AppViewArgs = {
  activeSectionId: ConfigEditorSectionId;
  state: ConfigState;
  derived: ConfigDerived;
  proxyService: ProxyServiceState;
  listActions: ConfigListActions;
  configActions: ConfigActions;
  proxyActions: ProxyServiceActions;
  onResetHotModelMappings: () => void;
};

type ConfigScreenController = Omit<AppViewArgs, "activeSectionId">;

const ConfigScreenControllerContext = createContext<ConfigScreenController | null>(null);

function buildAppViewProps({
  activeSectionId,
  state,
  derived,
  proxyService,
  listActions,
  configActions,
  proxyActions,
  onResetHotModelMappings,
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
    onResetHotModelMappings,
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

function useConfigScreenController(): ConfigScreenController {
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
  const {
    setStatus,
    setStatusMessage,
    updateForm,
  } = state;
  const resetHotModelMappings = useCallback(async () => {
    setStatus("loading");
    setStatusMessage("");
    try {
      const mappings = await invoke<Record<string, string>>("read_default_hot_model_mappings");
      const hotModelMappings = Object.entries(mappings)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([pattern, target]) => createModelMapping(pattern, target));
      updateForm({ hotModelMappings });
      setStatus("idle");
    } catch (error) {
      setStatus("error");
      setStatusMessage(parseError(error));
    }
  }, [setStatus, setStatusMessage, updateForm]);
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

  return {
    state,
    derived,
    proxyService,
    listActions,
    configActions,
    proxyActions,
    onResetHotModelMappings: resetHotModelMappings,
  };
}

/** 配置控制器由 `/config` layout 持有，leaf route 切换不会中断待保存草稿。 */
export function ConfigScreenProvider({ children }: ConfigScreenProviderProps) {
  const controller = useConfigScreenController();
  return (
    <ConfigScreenControllerContext.Provider value={controller}>
      {children}
    </ConfigScreenControllerContext.Provider>
  );
}

function ConfigScreenView({
  activeSectionId,
  controller,
}: ConfigScreenProps & { controller: ConfigScreenController }) {
  const appViewProps = buildAppViewProps({ activeSectionId, ...controller });

  return <AppView {...appViewProps} />;
}

function StandaloneConfigScreen({ activeSectionId }: ConfigScreenProps) {
  const controller = useConfigScreenController();
  return <ConfigScreenView activeSectionId={activeSectionId} controller={controller} />;
}

export function ConfigScreen({ activeSectionId }: ConfigScreenProps) {
  const controller = useContext(ConfigScreenControllerContext);
  if (!controller) {
    return <StandaloneConfigScreen activeSectionId={activeSectionId} />;
  }
  return <ConfigScreenView activeSectionId={activeSectionId} controller={controller} />;
}
