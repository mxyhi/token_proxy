export type CodexAccountStatus = "active" | "disabled" | "expired" | "invalid";
export type CodexAuthMethod = "oauth" | "agent_identity";

export type CodexAccountSummary = {
  account_id: string;
  email?: string | null;
  expires_at?: string | null;
  status: CodexAccountStatus;
  auth_method?: CodexAuthMethod;
  auto_refresh_enabled?: boolean;
  quota_threshold_percent?: number | null;
  proxy_url?: string | null;
  priority: number;
};

export type CodexLoginStatus = "waiting" | "success" | "error";

export type CodexLoginStartResponse = {
  state: string;
  login_url: string;
  interval_seconds: number;
  expires_at?: string | null;
};

export type CodexLoginPollResponse = {
  state: string;
  status: CodexLoginStatus;
  error?: string | null;
  account: CodexAccountSummary | null;
};

export type CodexQuotaItem = {
  name: string;
  percentage: number;
  used_percentage?: number | null;
  used: number | null;
  limit: number | null;
  reset_at: string | null;
};

export type CodexQuotaSummary = {
  account_id: string;
  plan_type: string | null;
  quotas: CodexQuotaItem[];
  error: string | null;
  checked_at: string | null;
};
