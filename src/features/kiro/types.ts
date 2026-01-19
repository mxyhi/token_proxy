export type KiroAccountStatus = "active" | "expired";

export type KiroAccountSummary = {
  account_id: string;
  provider: string;
  auth_method: string;
  email: string | null;
  expires_at: string | null;
  status: KiroAccountStatus;
};

export type KiroLoginMethod = "aws" | "aws_authcode" | "google";

export type KiroLoginStatus = "waiting" | "success" | "error";

export type KiroLoginStartResponse = {
  state: string;
  method: KiroLoginMethod;
  login_url: string | null;
  verification_uri: string | null;
  verification_uri_complete: string | null;
  user_code: string | null;
  interval_seconds: number | null;
  expires_at: string | null;
};

export type KiroLoginPollResponse = {
  state: string;
  status: KiroLoginStatus;
  error: string | null;
  account: KiroAccountSummary | null;
};

export type KiroQuotaItem = {
  name: string;
  percentage: number;
  used: number | null;
  limit: number | null;
  reset_at: string | null;
  is_trial: boolean;
};

export type KiroQuotaSummary = {
  account_id: string;
  provider: string;
  plan_type: string | null;
  quotas: KiroQuotaItem[];
  error: string | null;
};
