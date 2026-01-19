import { useCallback, useEffect, useState } from "react";

import { fetchKiroQuotas } from "@/features/kiro/api";
import type { KiroQuotaSummary } from "@/features/kiro/types";
import { parseError } from "@/lib/error";

export function useKiroQuotas() {
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
    void refresh();
  }, [refresh]);

  return { quotas, loading, error, refresh };
}
