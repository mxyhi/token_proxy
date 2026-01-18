import { useCallback, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { type StatusBadge } from "@/features/config/cards";
import {
  EMPTY_FORM,
  mergeConfigExtras,
  toPayload,
  validate,
} from "@/features/config/form";
import type {
  ConfigForm,
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

export type StatusState = "idle" | "loading" | "saving" | "saved" | "error";
export type AutoStartStatus = "idle" | "loading" | "error";

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

export function useConfigState() {
  const [form, setForm] = useState<ConfigForm>(EMPTY_FORM);
  const [configPath, setConfigPath] = useState("");
  const [lastConfig, setLastConfig] = useState<ProxyConfigFile | null>(null);
  const [configExtras, setConfigExtras] = useState<Record<string, unknown>>({});
  const [status, setStatus] = useState<StatusState>("idle");
  const [statusMessage, setStatusMessage] = useState("");
  const [savedAt, setSavedAt] = useState("");
  const [showLocalKey, setShowLocalKey] = useState(false);
  const [showUpstreamKeys, setShowUpstreamKeys] = useState(false);
  const [autoStartEnabled, setAutoStartEnabled] = useState(false);
  const [autoStartBaseline, setAutoStartBaseline] = useState(false);
  const [autoStartStatus, setAutoStartStatus] = useState<AutoStartStatus>("loading");
  const [autoStartMessage, setAutoStartMessage] = useState("");

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
    autoStartEnabled,
    autoStartBaseline,
    autoStartStatus,
    autoStartMessage,
    setConfigPath,
    setForm,
    setLastConfig,
    setConfigExtras,
    setSavedAt,
    setShowLocalKey,
    setShowUpstreamKeys,
    setAutoStartEnabled,
    setAutoStartBaseline,
    setAutoStartStatus,
    setAutoStartMessage,
    setStatus,
    setStatusMessage,
    updateForm,
  };
}

export function useProxyServiceState() {
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

export function useProxyServiceActions({
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

export function useConfigDerived(
  form: ConfigForm,
  lastConfig: ProxyConfigFile | null,
  configExtras: Record<string, unknown>,
  status: StatusState,
  autoStartEnabled: boolean,
  autoStartBaseline: boolean,
  autoStartStatus: AutoStartStatus
) {
  const { locale } = useI18n();
  const validation = useMemo(() => validate(form), [form, locale]);
  const currentPayload = useMemo(
    () => (validation.valid ? mergeConfigExtras(toPayload(form), configExtras) : null),
    [configExtras, form, validation.valid]
  );

  const configDirty = useMemo(() => {
    if (!currentPayload || !lastConfig) {
      return false;
    }
    return (
      stableStringify(currentPayload as JsonValue) !==
      stableStringify(lastConfig as JsonValue)
    );
  }, [currentPayload, lastConfig]);

  const autoStartDirty =
    autoStartStatus !== "loading" && autoStartEnabled !== autoStartBaseline;
  const isDirty = configDirty || autoStartDirty;

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

  return {
    validation,
    currentPayload,
    configDirty,
    autoStartDirty,
    isDirty,
    statusBadge,
    canSave,
    providerOptions,
  };
}
