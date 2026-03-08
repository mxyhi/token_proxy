import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { formatBytes, useUpdater, type UpdateStatus } from "@/features/update/updater";
import { m } from "@/paraglide/messages.js";

type ToastId = string | number;

// React StrictMode in dev will mount -> unmount -> mount components to surface side effects.
// Use a module-level guard to ensure we only auto-check once per app launch.
let didRunAutoCheck = false;

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
  const { checkForUpdate, downloadAndInstall, relaunchApp } = actions;
  const [dismissedRestartPromptKey, setDismissedRestartPromptKey] = useState<string | null>(null);
  const availableToastVersionRef = useRef<string | null>(null);
  const progressToastIdRef = useRef<ToastId | null>(null);
  const lastStatusRef = useRef<UpdateStatus>(state.status);
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

  useEffect(() => {
    if (didRunAutoCheck || !state.appProxyUrlReady) {
      return;
    }

    // Wait for config to load so app_proxy_url can be applied.
    didRunAutoCheck = true;
    void checkForUpdate({ source: "auto" });
  }, [checkForUpdate, state.appProxyUrlReady]);

  useEffect(() => {
    const previousStatus = lastStatusRef.current;
    lastStatusRef.current = state.status;

    if (state.status === "available" && state.lastCheckSource === "auto" && state.updateInfo) {
      const version = state.updateInfo.version;
      if (availableToastVersionRef.current !== version) {
        availableToastVersionRef.current = version;
        toast(m.update_status_available(), {
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
      }
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
  ]);

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
