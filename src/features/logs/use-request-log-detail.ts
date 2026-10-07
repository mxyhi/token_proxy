import { useEffect, useRef, useState } from "react";

import { parseError } from "@/lib/error";
import { readRequestLogBodyPage, readRequestLogDetail } from "./api";
import type { RequestLogDetail } from "./types";

export type DetailStatus = "idle" | "loading" | "error";

type PageTarget = { offset: number; previousOffsets: number[] };

export function useRequestLogDetail() {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<DetailStatus>("idle");
  const [message, setMessage] = useState("");
  const [detail, setDetail] = useState<RequestLogDetail | null>(null);
  const [page, setPage] = useState<PageTarget>({ offset: 0, previousOffsets: [] });
  const [pageLoading, setPageLoading] = useState(false);
  const [pageError, setPageError] = useState("");
  const [retryTarget, setRetryTarget] = useState<PageTarget | null>(null);
  const requestSeq = useRef(0);

  const resetPage = () => {
    setPage({ offset: 0, previousOffsets: [] });
    setPageLoading(false);
    setPageError("");
    setRetryTarget(null);
  };

  const select = async (id: number) => {
    const requestId = ++requestSeq.current;
    setOpen(true);
    setDetail(null);
    setStatus("loading");
    setMessage("");
    resetPage();
    try {
      const data = await readRequestLogDetail(id);
      if (requestSeq.current !== requestId) return;
      setDetail(data);
      setStatus("idle");
    } catch (error) {
      if (requestSeq.current !== requestId) return;
      setMessage(parseError(error));
      setStatus("error");
    }
  };

  const onOpenChange = (nextOpen: boolean) => {
    setOpen(nextOpen);
    if (nextOpen) return;
    requestSeq.current += 1;
    setDetail(null);
    setStatus("idle");
    setMessage("");
    resetPage();
  };

  const loadPage = async (target: PageTarget) => {
    if (!detail || pageLoading) return;
    const requestId = ++requestSeq.current;
    setPageLoading(true);
    setPageError("");
    setRetryTarget(target);
    try {
      const data = await readRequestLogBodyPage(detail.id, target.offset);
      if (requestSeq.current !== requestId) return;
      // 只替换当前页，历史保留偏移量；禁止把正文页拼接为全文。
      setDetail((current) => current && ({
        ...current,
        responseBody: data.text,
        responseBodyBytes: data.totalBytes,
        responseBodyNextOffset: data.nextOffset,
      }));
      setPage({ offset: data.offset, previousOffsets: target.previousOffsets });
      setRetryTarget(null);
    } catch (error) {
      if (requestSeq.current === requestId) setPageError(parseError(error));
    } finally {
      if (requestSeq.current === requestId) setPageLoading(false);
    }
  };

  const previousPage = () => {
    const offset = page.previousOffsets[page.previousOffsets.length - 1];
    if (offset === undefined) return;
    void loadPage({ offset, previousOffsets: page.previousOffsets.slice(0, -1) });
  };
  const nextPage = () => {
    const offset = detail?.responseBodyNextOffset;
    if (offset == null) return;
    void loadPage({ offset, previousOffsets: [...page.previousOffsets, page.offset] });
  };
  const retryPage = () => {
    if (retryTarget) void loadPage(retryTarget);
  };

  useEffect(() => () => { requestSeq.current += 1; }, []);

  return {
    open, status, message, detail, select, onOpenChange,
    offset: page.offset,
    hasPreviousPage: page.previousOffsets.length > 0,
    pageLoading, pageError, previousPage, nextPage, retryPage,
  };
}
