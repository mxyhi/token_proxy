import { useCallback, useEffect, useState } from "react";

import { fetchCodexQuotas } from "@/features/codex/api";
import type { CodexQuotaSummary } from "@/features/codex/types";
import { parseError } from "@/lib/error";

type UseCodexQuotasOptions = {
  autoLoad?: boolean;
};

export function useCodexQuotas(options?: UseCodexQuotasOptions) {
  const autoLoad = options?.autoLoad ?? true;
  const [quotas, setQuotas] = useState<CodexQuotaSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await fetchCodexQuotas();
      setQuotas(next);
      setError("");
    } catch (err) {
      setError(parseError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!autoLoad) {
      return;
    }
    void refresh();
  }, [autoLoad, refresh]);

  return { quotas, loading, error, refresh };
}
