import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { getSectionRoute } from "@/features/config/sections";
import {
  canStartUpdateCheck,
  formatBytes,
  useUpdater,
  type UpdateStatus,
} from "@/features/update/updater";
import { m } from "@/paraglide/messages.js";

type ToastId = string | number;
type VisibleWindowCheckState = {
  appProxyUrlReady: boolean;
  runAutoCheck: (reason: string) => void;
  status: UpdateStatus;
};

export const MAIN_WINDOW_VISIBLE_EVENT = "main-window-visible";

// React StrictMode in dev will mount -> unmount -> mount components to surface side effects.
// Use a module-level guard to ensure we only auto-check once per app launch.
let didRunAutoCheck = false;

export function __resetUpdateNotifierAutoCheckForTests() {
  didRunAutoCheck = false;
}

function buildDownloadProgressLabel(downloaded: number, total: number) {
  if (!total && !downloaded) {
    return "";
  }
  return m.update_download_progress({
    downloaded: formatBytes(downloaded),
    total: total ? formatBytes(total) : "--",
  });
}

export function UpdateNotifier() {
  const navigate = useNavigate();
  const { state, actions } = useUpdater();
  // MY-URL-COMPOSE PATCH G5：checkForUpdate 不再被自动路径调用（手动入口在 update-card.tsx）。
  const { downloadAndInstall, relaunchApp } = actions;
  const [dismissedRestartPromptKey, setDismissedRestartPromptKey] = useState<string | null>(null);
  const [windowVisible, setWindowVisible] = useState(false);
  const availableToastVersionRef = useRef<string | null>(null);
  const availableToastIdRef = useRef<ToastId | null>(null);
  const progressToastIdRef = useRef<ToastId | null>(null);
  const lastStatusRef = useRef<UpdateStatus>(state.status);
  const visibleWindowCheckRef = useRef<VisibleWindowCheckState>({
    appProxyUrlReady: state.appProxyUrlReady,
    runAutoCheck: () => undefined,
    status: state.status,
  });
  const installedRestartPromptKey =
    state.status === "installed"
      ? `${state.updateInfo?.version ?? "installed"}:${state.lastCheckedAt}`
      : null;
  const restartPromptOpen =
    installedRestartPromptKey !== null &&
    dismissedRestartPromptKey !== installedRestartPromptKey;

  const downloadProgressLabel = useMemo(
    () =>
      state.status === "downloading"
        ? buildDownloadProgressLabel(state.downloadState.downloaded, state.downloadState.total)
        : "",
    [state.downloadState.downloaded, state.downloadState.total, state.status]
  );

  const runAutoCheck = useCallback(
    (reason: string) => {
      // ══════════ MY-URL-COMPOSE PATCH G5 START ══════════
      // 关闭自动更新检测：启动 / 主窗口可见等自动触发全部短路。
      // 手动「检查更新」在 update-card.tsx 走 checkForUpdate({ source: "manual" })，不受影响。
      console.info("[updater] automatic update check disabled", { reason });
      // ══════════ MY-URL-COMPOSE PATCH G5 END ══════════
    },
    []
  );

  useLayoutEffect(() => {
    visibleWindowCheckRef.current = {
      appProxyUrlReady: state.appProxyUrlReady,
      runAutoCheck,
      status: state.status,
    };
  }, [runAutoCheck, state.appProxyUrlReady, state.status]);

  const triggerVisibleAutoCheck = useCallback((reason: string) => {
    const visibleState = visibleWindowCheckRef.current;
    if (!visibleState.appProxyUrlReady || !canStartUpdateCheck(visibleState.status)) {
      console.info("[updater] skip visible-window update check", {
        appProxyUrlReady: visibleState.appProxyUrlReady,
        status: visibleState.status,
        reason,
      });
      return;
    }

    visibleState.runAutoCheck(reason);
  }, []);

  useEffect(() => {
    if (didRunAutoCheck || !state.appProxyUrlReady) {
      return;
    }

    // Wait for config to load so app_proxy_url can be applied.
    didRunAutoCheck = true;
    runAutoCheck("startup");
  }, [runAutoCheck, state.appProxyUrlReady]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    const syncWindowVisibility = () => {
      const focused = document.hasFocus() && document.visibilityState !== "hidden";
      setWindowVisible(focused);
      if (focused) {
        triggerVisibleAutoCheck("main-window-focused");
      }
    };

    syncWindowVisibility();

    window.addEventListener("focus", syncWindowVisibility);
    document.addEventListener("visibilitychange", syncWindowVisibility);

    // Tauri emits this whenever the tray or single-instance path shows the main window.
    listen(MAIN_WINDOW_VISIBLE_EVENT, () => {
      syncWindowVisibility();
      triggerVisibleAutoCheck("main-window-visible");
    })
      .then((stopListening) => {
        if (disposed) {
          stopListening();
          return;
        }
        unlisten = stopListening;
      })
      .catch((error: unknown) => {
        console.warn("[updater] failed to listen for main window visibility", error);
      });

    return () => {
      disposed = true;
      window.removeEventListener("focus", syncWindowVisibility);
      document.removeEventListener("visibilitychange", syncWindowVisibility);
      if (unlisten) {
        unlisten();
      }
    };
  }, [triggerVisibleAutoCheck]);

  useEffect(() => {
    const previousStatus = lastStatusRef.current;
    lastStatusRef.current = state.status;

    if (
      windowVisible &&
      state.status === "available" &&
      state.lastCheckSource === "auto" &&
      state.updateInfo
    ) {
      const version = state.updateInfo.version;
      if (availableToastVersionRef.current !== version) {
        availableToastVersionRef.current = version;
        const toastId = toast(m.update_status_available(), {
          duration: Infinity,
          description: `${m.update_latest_version_label()}: ${version}`,
          action: {
            label: m.update_download_install(),
            onClick: () => {
              void downloadAndInstall();
            },
          },
          cancel: {
            label: m.update_toast_view_details(),
            onClick: () => {
              void navigate({ to: getSectionRoute("settings") });
            },
          },
        });
        availableToastIdRef.current = toastId;
      }
    }

    // Rechecks replace the update resource; remove stale available-update actions first.
    if (
      (state.status === "checking" ||
        state.status === "uptodate" ||
        state.status === "error" ||
        state.status === "downloading" ||
        state.status === "installing" ||
        state.status === "installed") &&
      availableToastIdRef.current
    ) {
      toast.dismiss(availableToastIdRef.current);
      availableToastIdRef.current = null;
      availableToastVersionRef.current = null;
    }

    if (state.status === "downloading" || state.status === "installing") {
      const title =
        state.status === "downloading"
          ? m.update_status_downloading()
          : m.update_status_installing();
      if (progressToastIdRef.current) {
        toast.loading(title, {
          id: progressToastIdRef.current,
          description: downloadProgressLabel,
          duration: Infinity,
        });
        return;
      }
      progressToastIdRef.current = toast.loading(title, {
        description: downloadProgressLabel,
        duration: Infinity,
      });
      return;
    }

    if (state.status === "installed" && previousStatus !== "installed") {
      if (progressToastIdRef.current) {
        toast.dismiss(progressToastIdRef.current);
        progressToastIdRef.current = null;
      }
      return;
    }

    if (state.status === "error") {
      if (previousStatus === "downloading" || previousStatus === "installing") {
        const toastId = progressToastIdRef.current;
        if (toastId) {
          toast.error(m.update_status_error(), {
            id: toastId,
            description: state.statusMessage || undefined,
            duration: 8000,
          });
          progressToastIdRef.current = null;
        } else {
          toast.error(m.update_status_error(), {
            description: state.statusMessage || undefined,
            duration: 8000,
          });
        }
      }
      return;
    }

    if (progressToastIdRef.current) {
      toast.dismiss(progressToastIdRef.current);
      progressToastIdRef.current = null;
    }
  }, [
    downloadAndInstall,
    downloadProgressLabel,
    navigate,
    state.lastCheckSource,
    state.lastCheckedAt,
    state.status,
    state.statusMessage,
    state.updateInfo,
    windowVisible,
  ]);

  useEffect(() => {
    return () => {
      if (availableToastIdRef.current) {
        toast.dismiss(availableToastIdRef.current);
      }
      if (progressToastIdRef.current) {
        toast.dismiss(progressToastIdRef.current);
      }
    };
  }, []);

  const handleRestartPromptOpenChange = useCallback(
    (open: boolean) => {
      if (!open && installedRestartPromptKey) {
        setDismissedRestartPromptKey(installedRestartPromptKey);
      }
    },
    [installedRestartPromptKey]
  );

  const onRestartNow = () => {
    if (installedRestartPromptKey) {
      setDismissedRestartPromptKey(installedRestartPromptKey);
    }
    void relaunchApp();
  };

  return (
    <div data-slot="update-notifier">
      <AlertDialog open={restartPromptOpen} onOpenChange={handleRestartPromptOpenChange}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{m.update_restart_prompt_title()}</AlertDialogTitle>
            <AlertDialogDescription>{m.update_restart_prompt_desc()}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{m.common_close()}</AlertDialogCancel>
            <AlertDialogAction type="button" onClick={onRestartNow}>
              {m.update_restart_now()}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
