import { useCallback, useEffect, useState } from "react";

import { fetchAntigravityQuotas } from "@/features/antigravity/api";
import type { AntigravityQuotaSummary } from "@/features/antigravity/types";
import { parseError } from "@/lib/error";

export function useAntigravityQuotas() {
  const [quotas, setQuotas] = useState<AntigravityQuotaSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await fetchAntigravityQuotas();
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
