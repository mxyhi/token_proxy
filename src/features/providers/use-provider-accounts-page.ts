import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { listProviderAccountsPage } from "@/features/providers/api";
import type { ProviderAccountsPage } from "@/features/providers/types";
import { parseError } from "@/lib/error";

export const PROVIDER_ACCOUNTS_PAGE_SIZE = 10;

export type ProviderAccountsPageFilters = {
  searchKeyword: string;
  providerFilter: "all" | "kiro" | "codex";
  statusFilter: "all" | "active" | "disabled" | "expired" | "cooling_down";
};

type ProviderAccountsPageStatus = "idle" | "loading" | "error";

function toProviderKind(value: ProviderAccountsPageFilters["providerFilter"]) {
  return value === "all" ? undefined : value;
}

function toStatus(value: ProviderAccountsPageFilters["statusFilter"]) {
  return value === "all" ? undefined : value;
}

export function useProviderAccountsPage(filters: ProviderAccountsPageFilters) {
  const [page, setPage] = useState(1);
  const [snapshot, setSnapshot] = useState<ProviderAccountsPage | null>(null);
  const [status, setStatus] = useState<ProviderAccountsPageStatus>("loading");
  const [error, setError] = useState("");
  const requestSeq = useRef(0);
  const filterKey = `${filters.searchKeyword}|${filters.providerFilter}|${filters.statusFilter}`;
  const lastFilterKey = useRef(filterKey);

  useEffect(() => {
    if (lastFilterKey.current === filterKey) {
      return;
    }
    lastFilterKey.current = filterKey;
    setPage(1);
  }, [filterKey]);

  const loadPage = useCallback(
    async (targetPage: number) => {
      const requestId = requestSeq.current + 1;
      requestSeq.current = requestId;
      setStatus("loading");
      setError("");
      try {
        const next = await listProviderAccountsPage({
          page: targetPage,
          pageSize: PROVIDER_ACCOUNTS_PAGE_SIZE,
          providerKind: toProviderKind(filters.providerFilter),
          status: toStatus(filters.statusFilter),
          search: filters.searchKeyword,
        });
        if (requestSeq.current !== requestId) {
          return;
        }
        setSnapshot(next);
        setStatus("idle");
      } catch (cause) {
        if (requestSeq.current !== requestId) {
          return;
        }
        setSnapshot(null);
        setStatus("error");
        setError(parseError(cause));
      }
    },
    [filters.providerFilter, filters.searchKeyword, filters.statusFilter]
  );

  useEffect(() => {
    const timerId = window.setTimeout(() => {
      void loadPage(page);
    }, 0);
    return () => window.clearTimeout(timerId);
  }, [loadPage, page]);

  const total = snapshot?.total ?? 0;
  const totalPages = useMemo(
    () => Math.max(1, Math.ceil(total / PROVIDER_ACCOUNTS_PAGE_SIZE)),
    [total]
  );

  const refresh = useCallback(async () => {
    await loadPage(page);
  }, [loadPage, page]);

  const resetPage = useCallback(() => {
    setPage(1);
  }, []);

  const onPrevPage = useCallback(() => {
    setPage((current) => Math.max(1, current - 1));
  }, []);

  const onNextPage = useCallback(() => {
    setPage((current) => Math.min(totalPages, current + 1));
  }, [totalPages]);

  return {
    items: snapshot?.items ?? [],
    total,
    page,
    pageSize: PROVIDER_ACCOUNTS_PAGE_SIZE,
    totalPages,
    loading: status === "loading",
    error,
    refresh,
    resetPage,
    onPrevPage,
    onNextPage,
  };
}
