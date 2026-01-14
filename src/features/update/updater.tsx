import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";

import { parseError } from "@/lib/error";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "uptodate"
  | "downloading"
  | "installing"
  | "installed"
  | "error";

export type UpdateInfo = {
  version: string;
  date?: string;
  body?: string;
};

export type DownloadState = {
  downloaded: number;
  total: number;
};

export type UpdateCheckSource = "auto" | "manual";

type UpdaterCheckResult = Awaited<ReturnType<typeof check>>;

type UpdateState = {
  status: UpdateStatus;
  statusMessage: string;
  lastCheckedAt: string;
  updateInfo: UpdateInfo | null;
  updateHandle: UpdaterCheckResult;
  downloadState: DownloadState;
  lastCheckSource: UpdateCheckSource | null;
  appProxyUrl: string;
  appProxyUrlReady: boolean;
};

type UpdateActions = {
  setAppProxyUrl: (value: string) => void;
  checkForUpdate: (args?: { source: UpdateCheckSource }) => Promise<void>;
  downloadAndInstall: () => Promise<void>;
  relaunchApp: () => Promise<void>;
};

type UpdaterContextValue = {
  state: UpdateState;
  actions: UpdateActions;
};

const UpdaterContext = createContext<UpdaterContextValue | null>(null);

export function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function toUpdateInfo(update: NonNullable<UpdaterCheckResult>): UpdateInfo {
  return {
    version: update.version,
    date: update.date,
    body: update.body,
  };
}

type UpdaterProviderProps = {
  children: ReactNode;
};

export function UpdaterProvider({ children }: UpdaterProviderProps) {
  const [state, setState] = useState<UpdateState>({
    status: "idle",
    statusMessage: "",
    lastCheckedAt: "",
    updateInfo: null,
    updateHandle: null,
    downloadState: { downloaded: 0, total: 0 },
    lastCheckSource: null,
    appProxyUrl: "",
    appProxyUrlReady: false,
  });

  const setAppProxyUrl = useCallback((value: string) => {
    setState((prev) => {
      if (prev.appProxyUrlReady && prev.appProxyUrl === value) {
        return prev;
      }
      return {
        ...prev,
        appProxyUrl: value,
        appProxyUrlReady: true,
      };
    });
  }, []);

  const checkForUpdate = useCallback(
    async (args?: { source: UpdateCheckSource }) => {
      const source = args?.source ?? "manual";

      setState((prev) => ({
        ...prev,
        status: "checking",
        statusMessage: "",
        lastCheckSource: source,
        updateInfo: null,
        updateHandle: null,
        downloadState: { downloaded: 0, total: 0 },
      }));

      try {
        const proxy = state.appProxyUrl.trim();
        const result = await check(proxy ? { proxy } : undefined);
        setState((prev) => ({
          ...prev,
          status: result ? "available" : "uptodate",
          updateInfo: result ? toUpdateInfo(result) : null,
          updateHandle: result,
          lastCheckedAt: new Date().toLocaleString(),
        }));
      } catch (error) {
        setState((prev) => ({
          ...prev,
          status: "error",
          statusMessage: parseError(error),
        }));
      }
    },
    [state.appProxyUrl]
  );

  const downloadAndInstall = useCallback(async () => {
    const updateHandle = state.updateHandle;
    if (!updateHandle) {
      return;
    }

    setState((prev) => ({
      ...prev,
      status: "downloading",
      statusMessage: "",
      downloadState: { downloaded: 0, total: 0 },
    }));

    const onProgress = (progress: DownloadEvent) => {
      if (progress.event === "Started") {
        setState((prev) => ({
          ...prev,
          downloadState: {
            downloaded: 0,
            total: progress.data?.contentLength ?? 0,
          },
        }));
        return;
      }
      if (progress.event === "Progress") {
        setState((prev) => ({
          ...prev,
          downloadState: {
            downloaded: prev.downloadState.downloaded + (progress.data?.chunkLength ?? 0),
            total: prev.downloadState.total,
          },
        }));
        return;
      }
      if (progress.event === "Finished") {
        setState((prev) => ({ ...prev, status: "installing" }));
      }
    };

    try {
      await updateHandle.downloadAndInstall(onProgress);
      setState((prev) => ({ ...prev, status: "installed" }));
    } catch (error) {
      setState((prev) => ({
        ...prev,
        status: "error",
        statusMessage: parseError(error),
      }));
    } finally {
      try {
        await updateHandle.close();
      } catch (_) {
        // ignore updater close errors to avoid masking update failures
      }
    }
  }, [state.updateHandle]);

  const relaunchApp = useCallback(async () => {
    setState((prev) => ({ ...prev, statusMessage: "" }));
    try {
      // Best-effort graceful shutdown before relaunching.
      try {
        await invoke<void>("prepare_relaunch");
      } catch (error) {
        setState((prev) => ({ ...prev, statusMessage: parseError(error) }));
      }
      await relaunch();
    } catch (error) {
      // 安装成功但重启失败时，不应把更新状态标记为失败；仅展示错误提示。
      setState((prev) => ({ ...prev, statusMessage: parseError(error) }));
    }
  }, []);

  const value = useMemo<UpdaterContextValue>(
    () => ({
      state,
      actions: { setAppProxyUrl, checkForUpdate, downloadAndInstall, relaunchApp },
    }),
    [checkForUpdate, downloadAndInstall, relaunchApp, setAppProxyUrl, state]
  );

  return <UpdaterContext.Provider value={value}>{children}</UpdaterContext.Provider>;
}

export function useUpdater() {
  const ctx = useContext(UpdaterContext);
  if (!ctx) {
    throw new Error("useUpdater must be used within an UpdaterProvider.");
  }
  return ctx;
}
