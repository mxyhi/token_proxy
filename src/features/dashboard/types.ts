export type DashboardRange = {
  fromTsMs: number | null;
  toTsMs: number | null;
};

export type DashboardSummary = {
  totalRequests: number;
  successRequests: number;
  errorRequests: number;
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
  provider: string;
  requests: number;
  totalTokens: number;
  cachedTokens: number;
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
  model: string | null;
  mappedModel: string | null;
  stream: boolean;
  status: number;
  totalTokens: number | null;
  cachedTokens: number | null;
  latencyMs: number;
  upstreamRequestId: string | null;
};

export type DashboardSnapshot = {
  summary: DashboardSummary;
  providers: DashboardProviderStat[];
  upstreams: DashboardUpstreamOption[];
  series: DashboardSeriesPoint[];
  recent: DashboardRequestItem[];
  truncated: boolean;
};

export type DashboardSnapshotQuery = {
  range: DashboardRange;
  offset?: number;
  upstreamId?: string | null;
};
