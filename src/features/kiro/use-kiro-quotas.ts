import { useCallback, useEffect, useState } from "react";

import { fetchKiroQuotas } from "@/features/kiro/api";
import type { KiroQuotaSummary } from "@/features/kiro/types";
import { parseError } from "@/lib/error";

type UseKiroQuotasOptions = {
  autoLoad?: boolean;
};

export function useKiroQuotas(options?: UseKiroQuotasOptions) {
  const autoLoad = options?.autoLoad ?? true;
  const [quotas, setQuotas] = useState<KiroQuotaSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await fetchKiroQuotas();
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
