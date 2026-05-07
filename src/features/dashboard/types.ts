export type DashboardRange = {
  fromTsMs: number | null;
  toTsMs: number | null;
};

export type DashboardSummary = {
  totalRequests: number;
  successRequests: number;
  errorRequests: number;
  costNanoUsd: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  avgLatencyMs: number;
  medianLatencyMs: number;
};

export type DashboardProviderStat = {
  provider: string;
  requests: number;
  totalTokens: number;
  cachedTokens: number;
};

export type DashboardUpstreamOption = {
  upstreamId: string;
  requests: number;
  totalTokens: number;
  cachedTokens: number;
};

export type DashboardAccountOption = {
  upstreamId: string;
  accountId: string | null;
  requests: number;
  totalTokens: number;
  cachedTokens: number;
};

export type DashboardUpstreamModelProbeStatus =
  | "pending"
  | "ok"
  | "failed"
  | "unsupported";

export type DashboardUpstreamModelProbe = {
  upstreamId: string;
  provider: string;
  accountId: string | null;
  status: DashboardUpstreamModelProbeStatus;
  checkedAtTsMs: number | null;
  error: string | null;
  models: string[];
};

export type DashboardSeriesPoint = {
  tsMs: number;
  totalRequests: number;
  errorRequests: number;
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  totalTokens: number;
};

export type DashboardRequestItem = {
  id: number;
  tsMs: number;
  path: string;
  provider: string;
  upstreamId: string;
  accountId?: string | null;
  model: string | null;
  mappedModel: string | null;
  stream: boolean;
  status: number;
  totalTokens: number | null;
  outputTokens: number | null;
  cachedTokens: number | null;
  costNanoUsd: number | null;
  pricingVersion: string | null;
  pricingModel: string | null;
  pricingContextTier: string | null;
  latencyMs: number;
  upstreamFirstByteMs?: number | null;
  upstreamResponseHeadersMs?: number | null;
  upstreamFirstBodyChunkMs?: number | null;
  firstClientFlushMs?: number | null;
  firstOutputMs?: number | null;
  upstreamRequestId: string | null;
};

export type DashboardSnapshot = {
  summary: DashboardSummary;
  providers: DashboardProviderStat[];
  upstreams: DashboardUpstreamOption[];
  accounts: DashboardAccountOption[];
  series: DashboardSeriesPoint[];
  recent: DashboardRequestItem[];
  modelProbes: DashboardUpstreamModelProbe[];
  truncated: boolean;
};

export type DashboardSnapshotQuery = {
  range: DashboardRange;
  offset?: number;
  upstreamId?: string | null;
  accountId?: string | null;
  publicOnly?: boolean;
};
