import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import { getVersion } from "@tauri-apps/api/app";
import { AlertCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  formatBytes,
  useUpdater,
  type DownloadState,
  type UpdateInfo,
  type UpdateStatus,
} from "@/features/update/updater";
import { m } from "@/paraglide/messages.js";

type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

function resolveStatusBadge(status: UpdateStatus) {
  let label = m.update_status_idle();
  let variant: BadgeVariant = "outline";

  switch (status) {
    case "checking":
      label = m.update_status_checking();
      variant = "secondary";
      break;
    case "available":
      label = m.update_status_available();
      variant = "default";
      break;
    case "uptodate":
      label = m.update_status_uptodate();
      variant = "outline";
      break;
    case "downloading":
      label = m.update_status_downloading();
      variant = "secondary";
      break;
    case "installing":
      label = m.update_status_installing();
      variant = "secondary";
      break;
    case "installed":
      label = m.update_status_installed();
      variant = "default";
      break;
    case "error":
      label = m.update_status_error();
      variant = "destructive";
      break;
    default:
      break;
  }

  return { label, variant };
}

function useAppVersion() {
  const [currentVersion, setCurrentVersion] = useState("");

  useEffect(() => {
    let cancelled = false;
    void getVersion()
      .then((version) => {
        if (!cancelled) {
          setCurrentVersion(version);
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  return currentVersion;
}

type UpdateStatusRowProps = {
  currentVersion: string;
  badge: { label: string; variant: BadgeVariant };
  lastCheckedAt: string;
};

function UpdateStatusRow({ currentVersion, badge, lastCheckedAt }: UpdateStatusRowProps) {
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <span className="text-muted-foreground">{m.update_current_version_label()}</span>
        <span className="font-mono text-xs text-foreground/80">
          {currentVersion || "--"}
        </span>
        <Badge variant={badge.variant}>{badge.label}</Badge>
      </div>
      {lastCheckedAt ? (
        <p className="text-xs text-muted-foreground">
          {m.update_last_checked({ time: lastCheckedAt })}
        </p>
      ) : null}
    </div>
  );
}

type UpdateDetailsProps = {
  status: UpdateStatus;
  updateInfo: UpdateInfo | null;
};

function UpdateDetails({ status, updateInfo }: UpdateDetailsProps) {
  if (!updateInfo) {
    const message = resolveStatusBadge(status).label;
    return <p className="text-sm text-muted-foreground">{message}</p>;
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground">{m.update_latest_version_label()}</span>
        <span className="font-mono text-xs text-foreground/80">{updateInfo.version}</span>
      </div>
      {updateInfo.date ? (
        <div className="text-xs text-muted-foreground">
          {m.update_release_date_label()} {updateInfo.date}
        </div>
      ) : null}
      <div>
        <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
          {m.update_release_notes_label()}
        </p>
        <div className="mt-2 rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground whitespace-pre-wrap">
          {updateInfo.body || m.update_release_notes_empty()}
        </div>
      </div>
    </div>
  );
}

type UpdateProgressProps = {
  label: string;
};

function UpdateProgress({ label }: UpdateProgressProps) {
  if (!label) {
    return null;
  }
  return <div className="text-xs text-muted-foreground">{label}</div>;
}

type UpdateErrorProps = {
  message: string;
};

function UpdateError({ message }: UpdateErrorProps) {
  if (!message) {
    return null;
  }
  return (
    <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-xs text-destructive">
      <div className="flex items-center gap-2">
        <AlertCircle className="size-4" aria-hidden="true" />
        <span>{message}</span>
      </div>
    </div>
  );
}

type UpdateActionsProps = {
  canCheck: boolean;
  canInstall: boolean;
  canRelaunch: boolean;
  onCheck: () => void;
  onInstall: () => void;
  onRelaunch: () => void;
};

function UpdateActions({
  canCheck,
  canInstall,
  canRelaunch,
  onCheck,
  onInstall,
  onRelaunch,
}: UpdateActionsProps) {
  return (
    <div className="flex flex-wrap gap-2">
      <Button type="button" variant="outline" onClick={onCheck} disabled={!canCheck}>
        {m.update_check()}
      </Button>
      <Button type="button" onClick={onInstall} disabled={!canInstall}>
        {m.update_download_install()}
      </Button>
      {canRelaunch ? (
        <Button type="button" variant="secondary" onClick={onRelaunch}>
          {m.update_restart_now()}
        </Button>
      ) : null}
    </div>
  );
}

function resolveProgressLabel(status: UpdateStatus, downloadState: DownloadState) {
  if (status !== "downloading") {
    return "";
  }
  const total = downloadState.total;
  const downloaded = downloadState.downloaded;
  if (!total && !downloaded) {
    return "";
  }
  return m.update_download_progress({
    downloaded: formatBytes(downloaded),
    total: total ? formatBytes(total) : "--",
  });
}

export function UpdateCard() {
  const currentVersion = useAppVersion();
  const { state, actions } = useUpdater();
  const statusBadge = useMemo(() => resolveStatusBadge(state.status), [state.status]);
  const progressLabel = useMemo(
    () => resolveProgressLabel(state.status, state.downloadState),
    [state.downloadState, state.status]
  );
  const canCheck =
    state.status !== "checking" && state.status !== "downloading" && state.status !== "installing";
  const canInstall = state.status === "available" && !!state.updateHandle;
  const canRelaunch = state.status === "installed";
  const triggerManualCheck = useCallback(() => {
    void actions.checkForUpdate({ source: "manual" });
  }, [actions]);

  return (
    <Card data-slot="update-card">
      <CardHeader>
        <CardTitle>{m.update_title()}</CardTitle>
        <CardDescription>{m.update_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <UpdateStatusRow
          currentVersion={currentVersion}
          badge={statusBadge}
          lastCheckedAt={state.lastCheckedAt}
        />
        <Separator />
        <UpdateDetails status={state.status} updateInfo={state.updateInfo} />
        <UpdateProgress label={progressLabel} />
        <UpdateError message={state.statusMessage} />
      </CardContent>
      <CardFooter>
        <UpdateActions
          canCheck={canCheck}
          canInstall={canInstall}
          canRelaunch={canRelaunch}
          onCheck={triggerManualCheck}
          onInstall={actions.downloadAndInstall}
          onRelaunch={actions.relaunchApp}
        />
      </CardFooter>
    </Card>
  );
}
