export type ProviderAccountQuotaItem = {
  name: string;
  percentage: number;
  used: number | null;
  limit: number | null;
  reset_at: string | null;
  is_trial: boolean;
};

export type ProviderAccountQuotaSnapshot = {
  plan_type: string | null;
  error: string | null;
  checked_at: string | null;
  items: ProviderAccountQuotaItem[];
};

export type ProviderAccountPageItem = {
  provider_kind: "kiro" | "codex";
  account_id: string;
  email?: string | null;
  expires_at?: string | null;
  status: "active" | "disabled" | "expired" | "cooling_down";
  auth_method?: string | null;
  provider_name?: string | null;
  auto_refresh_enabled?: boolean | null;
  proxy_url?: string | null;
  quota: ProviderAccountQuotaSnapshot;
};

export type ProviderAccountsPage = {
  items: ProviderAccountPageItem[];
  total: number;
  page: number;
  page_size: number;
};
