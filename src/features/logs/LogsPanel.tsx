import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertCircle } from "lucide-react";

import { DataTable } from "@/components/data-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Switch } from "@/components/ui/switch";
import {
  DashboardFilters,
  RECENT_PAGE_SIZE,
  useDashboardSnapshot,
} from "@/features/dashboard/snapshot";
import {
  readRequestDetailCapture,
  readRequestLogDetail,
  setRequestDetailCapture,
} from "@/features/logs/api";
import type { RequestLogDetail } from "@/features/logs/types";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

const DETAIL_PLACEHOLDER = "—";
const REQUEST_DETAIL_CAPTURE_EVENT = "request-detail-capture-changed";

type DetailStatus = "idle" | "loading" | "error";

type RequestDetailCaptureEvent = {
  enabled: boolean;
};

type DetailSectionProps = {
  title: string;
  value: string | null;
};

function DetailSection({ title, value }: DetailSectionProps) {
  const content = value?.trim() ? value : null;
  return (
    <div className="space-y-2">
      <p className="text-sm font-medium text-foreground">{title}</p>
      {content ? (
        <pre className="rounded-lg border border-border/60 bg-muted/20 p-3 text-xs whitespace-pre-wrap break-words">
          {content}
        </pre>
      ) : (
        <p className="text-xs text-muted-foreground">{DETAIL_PLACEHOLDER}</p>
      )}
    </div>
  );
}

type RequestDetailSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  status: DetailStatus;
  statusMessage: string;
  detail: RequestLogDetail | null;
};

function RequestDetailSheet({
  open,
  onOpenChange,
  status,
  statusMessage,
  detail,
}: RequestDetailSheetProps) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>{m.logs_detail_title()}</SheetTitle>
          <SheetDescription>{m.logs_detail_desc()}</SheetDescription>
        </SheetHeader>
        <ScrollArea className="flex-1">
          <div className="space-y-4 px-4 pb-6">
            {status === "loading" ? (
              <p className="text-sm text-muted-foreground">{m.logs_detail_loading()}</p>
            ) : null}
            {status === "error" ? (
              <Alert variant="destructive">
                <AlertCircle className="size-4" aria-hidden="true" />
                <div>
                  <AlertTitle>{m.logs_detail_error()}</AlertTitle>
                  <AlertDescription>{statusMessage}</AlertDescription>
                </div>
              </Alert>
            ) : null}
            {status === "idle" ? (
              <div className="space-y-4">
                <DetailSection
                  title={m.logs_detail_headers()}
                  value={detail?.requestHeaders ?? null}
                />
                <DetailSection
                  title={m.logs_detail_body()}
                  value={detail?.requestBody ?? null}
                />
                <DetailSection
                  title={m.logs_detail_response()}
                  value={detail?.responseError ?? null}
                />
              </div>
            ) : null}
          </div>
        </ScrollArea>
      </SheetContent>
    </Sheet>
  );
}

export function LogsPanel() {
  const {
    snapshot,
    status,
    statusMessage,
    rangePreset,
    pagination,
    refresh,
    onRangeChange,
    onPrevPage,
    onNextPage,
  } = useDashboardSnapshot();

  const [captureEnabled, setCaptureEnabled] = useState(false);
  const [captureLoading, setCaptureLoading] = useState(false);
  const [captureMessage, setCaptureMessage] = useState("");
  const [detailOpen, setDetailOpen] = useState(false);
  const [detailStatus, setDetailStatus] = useState<DetailStatus>("idle");
  const [detailMessage, setDetailMessage] = useState("");
  const [detail, setDetail] = useState<RequestLogDetail | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);

  const isLoading = status === "loading";

  const loadCaptureState = useCallback(async () => {
    try {
      const enabled = await readRequestDetailCapture();
      setCaptureEnabled(enabled);
      setCaptureMessage("");
    } catch (error) {
      setCaptureMessage(parseError(error));
    }
  }, []);

  useEffect(() => {
    void loadCaptureState();
  }, [loadCaptureState]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;

    const setupListener = async () => {
      try {
        const stop = await listen<RequestDetailCaptureEvent>(
          REQUEST_DETAIL_CAPTURE_EVENT,
          (event) => {
            if (!active) {
              return;
            }
            setCaptureEnabled(event.payload.enabled);
          }
        );
        if (!active) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (error) {
        if (!active) {
          return;
        }
        setCaptureMessage(parseError(error));
      }
    };

    void setupListener();

    return () => {
      active = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  useEffect(() => {
    if (status === "idle") {
      void loadCaptureState();
    }
  }, [status, snapshot, loadCaptureState]);

  const handleToggleCapture = useCallback(async (nextValue: boolean) => {
    setCaptureLoading(true);
    setCaptureMessage("");
    try {
      const enabled = await setRequestDetailCapture(nextValue);
      setCaptureEnabled(enabled);
    } catch (error) {
      setCaptureMessage(parseError(error));
    } finally {
      setCaptureLoading(false);
    }
  }, []);

  const handleSelectItem = useCallback((itemId: number) => {
    setSelectedId(itemId);
    setDetailOpen(true);
  }, []);

  const loadDetail = useCallback(async (itemId: number) => {
    setDetailStatus("loading");
    setDetailMessage("");
    try {
      const data = await readRequestLogDetail(itemId);
      setDetail(data);
      setDetailStatus("idle");
    } catch (error) {
      setDetailMessage(parseError(error));
      setDetailStatus("error");
    }
  }, []);

  useEffect(() => {
    if (!detailOpen) {
      setDetail(null);
      setDetailStatus("idle");
      setDetailMessage("");
      return;
    }
    if (selectedId !== null) {
      void loadDetail(selectedId);
    }
  }, [detailOpen, selectedId, loadDetail]);

  const captureStatusText = captureEnabled
    ? m.logs_capture_status_ready()
    : m.logs_capture_status_idle();

  return (
    <div className="flex flex-col gap-4">
      {status === "error" ? (
        <Alert variant="destructive" className="mx-4 lg:mx-6">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.dashboard_load_failed()}</AlertTitle>
            <AlertDescription>{statusMessage}</AlertDescription>
          </div>
        </Alert>
      ) : null}

      <div className="px-4 lg:px-6">
        <Card className="border-border/60 bg-background/70">
          <CardHeader className="pb-2">
            <CardTitle className="text-base">{m.logs_capture_title()}</CardTitle>
            <CardDescription>{m.logs_capture_desc()}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-wrap items-center justify-between gap-3">
            <div className="space-y-1">
              <Label htmlFor="logs-capture" className="text-xs text-muted-foreground">
                {captureStatusText}
              </Label>
              {captureMessage ? (
                <p className="text-xs text-destructive">{captureMessage}</p>
              ) : null}
            </div>
            <Switch
              id="logs-capture"
              checked={captureEnabled}
              disabled={captureLoading}
              onCheckedChange={handleToggleCapture}
            />
          </CardContent>
        </Card>
      </div>

      <DashboardFilters
        range={rangePreset}
        loading={isLoading}
        onRangeChange={onRangeChange}
        onRefresh={refresh}
      />

      <DataTable
        items={snapshot?.recent ?? []}
        page={pagination.page}
        totalPages={pagination.totalPages}
        totalRequests={pagination.totalRequests}
        pageSize={RECENT_PAGE_SIZE}
        loading={isLoading}
        scrollKey={`${rangePreset}-${pagination.page}`}
        onPrevPage={onPrevPage}
        onNextPage={onNextPage}
        onSelectItem={(item) => handleSelectItem(item.id)}
      />

      <RequestDetailSheet
        open={detailOpen}
        onOpenChange={setDetailOpen}
        status={detailStatus}
        statusMessage={detailMessage}
        detail={detail}
      />
    </div>
  );
}
