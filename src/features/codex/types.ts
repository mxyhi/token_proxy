export type CodexAccountStatus = "active" | "disabled" | "expired";

export type CodexAccountSummary = {
  account_id: string;
  email?: string | null;
  expires_at?: string | null;
  status: CodexAccountStatus;
  auto_refresh_enabled?: boolean;
  proxy_url?: string | null;
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
  used: number | null;
  limit: number | null;
  reset_at: string | null;
};

export type CodexQuotaSummary = {
  account_id: string;
  plan_type: string | null;
  quotas: CodexQuotaItem[];
  error: string | null;
};
