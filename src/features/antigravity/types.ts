export type AntigravityAccountStatus = "active" | "expired";

export type AntigravityAccountSummary = {
  account_id: string;
  email?: string | null;
  expires_at?: string | null;
  status: AntigravityAccountStatus;
  source?: string | null;
};

export type AntigravityLoginStatus = "waiting" | "success" | "error";

export type AntigravityLoginStartResponse = {
  state: string;
  login_url: string;
  interval_seconds: number;
  expires_at?: string | null;
};

export type AntigravityLoginPollResponse = {
  state: string;
  status: AntigravityLoginStatus;
  error?: string | null;
  account: AntigravityAccountSummary | null;
};

export type AntigravityQuotaItem = {
  name: string;
  percentage: number;
  reset_at: string | null;
};

export type AntigravityQuotaSummary = {
  account_id: string;
  plan_type: string | null;
  quotas: AntigravityQuotaItem[];
  error: string | null;
};

export type AntigravityIdeStatus = {
  database_available: boolean;
  ide_running: boolean;
  active_email?: string | null;
};

export type AntigravityWarmupScheduleSummary = {
  account_id: string;
  model: string;
  interval_minutes: number;
  next_run_at?: string | null;
  enabled: boolean;
};
