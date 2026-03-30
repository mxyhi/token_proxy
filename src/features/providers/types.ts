export type ProviderAccountPageItem = {
  provider_kind: "kiro" | "codex";
  account_id: string;
  email?: string | null;
  expires_at?: string | null;
  status: "active" | "expired";
  auth_method?: string | null;
  provider_name?: string | null;
  auto_refresh_enabled?: boolean | null;
  proxy_url?: string | null;
};

export type ProviderAccountsPage = {
  items: ProviderAccountPageItem[];
  total: number;
  page: number;
  page_size: number;
};
