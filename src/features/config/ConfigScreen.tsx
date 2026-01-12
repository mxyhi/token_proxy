import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { AppView } from "@/features/config/AppView";
import { type StatusBadge } from "@/features/config/cards";
import {
  EMPTY_FORM,
  extractConfigExtras,
  mergeConfigExtras,
  toForm,
  toPayload,
  validate,
} from "@/features/config/form";
import { useConfigListActions } from "@/features/config/list-actions";
import type { ConfigSectionId } from "@/features/config/sections";
import type {
  ConfigForm,
  ConfigResponse,
  ProxyConfigFile,
  ProxyServiceRequestState,
  ProxyServiceStatus,
} from "@/features/config/types";
import { parseError } from "@/lib/error";
import { useI18n } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

type JsonValue = null | string | number | boolean | JsonValue[] | { [key: string]: JsonValue };

function sortJsonValue(value: JsonValue): JsonValue {
  if (Array.isArray(value)) {
    return value.map((item) => sortJsonValue(item));
  }
  if (value && typeof value === "object") {
    const sorted: { [key: string]: JsonValue } = {};
    for (const key of Object.keys(value).sort()) {
      sorted[key] = sortJsonValue(value[key]);
    }
    return sorted;
  }
  return value;
}

function stableStringify(value: JsonValue) {
  return JSON.stringify(sortJsonValue(value));
}

type StatusState = "idle" | "loading" | "saving" | "saved" | "error";

type ConfigScreenProps = {
  activeSectionId: ConfigSectionId;
};

function createStatusBadge(
  status: StatusState,
  isDirty: boolean,
  lastConfig: ProxyConfigFile | null
): StatusBadge {
  if (status === "loading" || status === "saving") {
    return { id: "working", label: m.config_status_working(), variant: "secondary" };
  }
  if (status === "error") {
    return { id: "error", label: m.config_status_error(), variant: "destructive" };
  }
  if (isDirty) {
    return { id: "unsaved", label: m.config_status_unsaved(), variant: "secondary" };
  }
  if (lastConfig) {
    return { id: "saved", label: m.config_status_saved(), variant: "default" };
  }
  return { id: "idle", label: m.config_status_idle(), variant: "outline" };
}

function useConfigState() {
  const [form, setForm] = useState<ConfigForm>(EMPTY_FORM);
  const [configPath, setConfigPath] = useState("");
  const [lastConfig, setLastConfig] = useState<ProxyConfigFile | null>(null);
  const [configExtras, setConfigExtras] = useState<Record<string, unknown>>({});
  const [status, setStatus] = useState<StatusState>("idle");
  const [statusMessage, setStatusMessage] = useState("");
  const [savedAt, setSavedAt] = useState("");
  const [showLocalKey, setShowLocalKey] = useState(false);
  const [showUpstreamKeys, setShowUpstreamKeys] = useState(false);

  const updateForm = useCallback((patch: Partial<ConfigForm>) => {
    setForm((prev) => ({ ...prev, ...patch }));
  }, []);

  return {
    form,
    configPath,
    lastConfig,
    configExtras,
    savedAt,
    showLocalKey,
    showUpstreamKeys,
    status,
    statusMessage,
    setConfigPath,
    setForm,
    setLastConfig,
    setConfigExtras,
    setSavedAt,
    setShowLocalKey,
    setShowUpstreamKeys,
    setStatus,
    setStatusMessage,
    updateForm,
  };
}

function useProxyServiceState() {
  const [proxyServiceStatus, setProxyServiceStatus] = useState<ProxyServiceStatus | null>(null);
  const [proxyServiceRequestState, setProxyServiceRequestState] =
    useState<ProxyServiceRequestState>("idle");
  const [proxyServiceMessage, setProxyServiceMessage] = useState("");

  return {
    proxyServiceStatus,
    proxyServiceRequestState,
    proxyServiceMessage,
    setProxyServiceStatus,
    setProxyServiceRequestState,
    setProxyServiceMessage,
  };
}

type ProxyServiceActionsArgs = {
  setProxyServiceStatus: (value: ProxyServiceStatus) => void;
  setProxyServiceRequestState: (value: ProxyServiceRequestState) => void;
  setProxyServiceMessage: (value: string) => void;
};

function useProxyServiceActions({
  setProxyServiceStatus,
  setProxyServiceRequestState,
  setProxyServiceMessage,
}: ProxyServiceActionsArgs) {
  const refreshProxyStatus = useCallback(async () => {
    setProxyServiceRequestState("working");
    setProxyServiceMessage("");
    try {
      const status = await invoke<ProxyServiceStatus>("proxy_status");
      setProxyServiceStatus(status);
      setProxyServiceRequestState("idle");
    } catch (error) {
      setProxyServiceRequestState("error");
      setProxyServiceMessage(parseError(error));
    }
  }, [setProxyServiceMessage, setProxyServiceRequestState, setProxyServiceStatus]);

  const runProxyCommand = useCallback(
    async (command: "proxy_start" | "proxy_stop" | "proxy_restart" | "proxy_reload") => {
      setProxyServiceRequestState("working");
      setProxyServiceMessage("");
      try {
        const status = await invoke<ProxyServiceStatus>(command);
        setProxyServiceStatus(status);
        setProxyServiceRequestState("idle");
      } catch (error) {
        setProxyServiceRequestState("error");
        setProxyServiceMessage(parseError(error));
      }
    },
    [setProxyServiceMessage, setProxyServiceRequestState, setProxyServiceStatus]
  );

  const startProxy = useCallback(async () => runProxyCommand("proxy_start"), [runProxyCommand]);
  const stopProxy = useCallback(async () => runProxyCommand("proxy_stop"), [runProxyCommand]);
  const restartProxy = useCallback(async () => runProxyCommand("proxy_restart"), [runProxyCommand]);
  const reloadProxy = useCallback(async () => runProxyCommand("proxy_reload"), [runProxyCommand]);

  return { refreshProxyStatus, startProxy, stopProxy, restartProxy, reloadProxy };
}

function useConfigDerived(
  form: ConfigForm,
  lastConfig: ProxyConfigFile | null,
  configExtras: Record<string, unknown>,
  status: StatusState
) {
  const { locale } = useI18n();
  const validation = useMemo(() => validate(form), [form, locale]);
  const currentPayload = useMemo(
    () => (validation.valid ? mergeConfigExtras(toPayload(form), configExtras) : null),
    [configExtras, form, validation.valid]
  );

  const isDirty = useMemo(() => {
    if (!currentPayload || !lastConfig) {
      return false;
    }
    return (
      stableStringify(currentPayload as JsonValue) !==
      stableStringify(lastConfig as JsonValue)
    );
  }, [currentPayload, lastConfig]);

  const statusBadge = createStatusBadge(status, isDirty, lastConfig);

  const providerOptions = useMemo(() => {
    const providers = new Set<string>();
    for (const upstream of form.upstreams) {
      const provider = upstream.provider.trim();
      if (provider) {
        providers.add(provider);
      }
    }
    return Array.from(providers);
  }, [form.upstreams]);

  const canSave = status !== "saving" && validation.valid && isDirty;

  return { validation, currentPayload, isDirty, statusBadge, canSave, providerOptions };
}

type ConfigActionsArgs = {
  currentPayload: ProxyConfigFile | null;
  validation: { valid: boolean; message: string };
  setConfigPath: (path: string) => void;
  setForm: (value: ConfigForm) => void;
  setLastConfig: (value: ProxyConfigFile | null) => void;
  setConfigExtras: (extras: Record<string, unknown>) => void;
  setSavedAt: (value: string) => void;
  setStatus: (value: StatusState) => void;
  setStatusMessage: (value: string) => void;
  setProxyServiceStatus: (value: ProxyServiceStatus) => void;
  setProxyServiceMessage: (value: string) => void;
};

type LoadConfigArgs = Pick<
  ConfigActionsArgs,
  "setConfigPath" | "setForm" | "setLastConfig" | "setStatus" | "setStatusMessage"
> & {
  setConfigExtras: (extras: Record<string, unknown>) => void;
  setSavedAt: (value: string) => void;
};

async function loadConfigImpl({
  setConfigPath,
  setForm,
  setLastConfig,
  setConfigExtras,
  setStatus,
  setStatusMessage,
  setSavedAt,
}: LoadConfigArgs) {
  setStatus("loading");
  setStatusMessage("");
  try {
    const response = await invoke<ConfigResponse>("read_proxy_config");
    setConfigPath(response.path);
    setForm(toForm(response.config));
    setConfigExtras(extractConfigExtras(response.config));
    setLastConfig(response.config);
    setSavedAt("");
    setStatus("idle");
  } catch (error) {
    setStatus("error");
    setStatusMessage(parseError(error));
  }
}

type SaveConfigArgs = Pick<
  ConfigActionsArgs,
  | "setLastConfig"
  | "setSavedAt"
  | "setStatus"
  | "setStatusMessage"
  | "setProxyServiceStatus"
  | "setProxyServiceMessage"
> & {
  currentPayload: ProxyConfigFile | null;
  validation: { valid: boolean; message: string };
};

async function saveConfigImpl({
  currentPayload,
  validation,
  setLastConfig,
  setSavedAt,
  setStatus,
  setStatusMessage,
  setProxyServiceStatus,
  setProxyServiceMessage,
}: SaveConfigArgs) {
  if (!currentPayload) {
    setStatus("error");
    setStatusMessage(validation.message || m.config_invalid_configuration());
    return;
  }
  setStatus("saving");
  setStatusMessage("");
  setProxyServiceMessage("");
  try {
    const status = await invoke<ProxyServiceStatus>("write_proxy_config", { config: currentPayload });
    setProxyServiceStatus(status);
    setLastConfig(currentPayload);
    setSavedAt(new Date().toLocaleString());
    setStatus("saved");
  } catch (error) {
    setStatus("error");
    setStatusMessage(parseError(error));
    setProxyServiceMessage(parseError(error));
  }
}

function useConfigActions({
  currentPayload,
  validation,
  setConfigPath,
  setForm,
  setLastConfig,
  setConfigExtras,
  setSavedAt,
  setStatus,
  setStatusMessage,
  setProxyServiceStatus,
  setProxyServiceMessage,
}: ConfigActionsArgs) {
  const loadConfig = useCallback(
    () =>
      loadConfigImpl({
        setConfigPath,
        setForm,
        setLastConfig,
        setConfigExtras,
        setStatus,
        setStatusMessage,
        setSavedAt,
      }),
    [
      setConfigPath,
      setForm,
      setLastConfig,
      setConfigExtras,
      setStatus,
      setStatusMessage,
      setSavedAt,
    ]
  );
  const saveConfig = useCallback(
    () =>
      saveConfigImpl({
        currentPayload,
        validation,
        setLastConfig,
        setSavedAt,
        setStatus,
        setStatusMessage,
        setProxyServiceStatus,
        setProxyServiceMessage,
      }),
    [
      currentPayload,
      setLastConfig,
      setProxyServiceMessage,
      setProxyServiceStatus,
      setSavedAt,
      setStatus,
      setStatusMessage,
      validation.message,
    ]
  );

  return { loadConfig, saveConfig };
}

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
    state.status
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
    setConfigPath: state.setConfigPath,
    setForm: state.setForm,
    setLastConfig: state.setLastConfig,
    setConfigExtras: state.setConfigExtras,
    setSavedAt: state.setSavedAt,
    setStatus: state.setStatus,
    setStatusMessage: state.setStatusMessage,
    setProxyServiceStatus: proxyService.setProxyServiceStatus,
    setProxyServiceMessage: proxyService.setProxyServiceMessage,
  });
  const { loadConfig } = configActions;
  const listActions = useConfigListActions(state.setForm);

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

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
