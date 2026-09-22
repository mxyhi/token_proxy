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
  uncachedInputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cacheWrite5mTokens?: number;
  cacheWrite1hTokens?: number;
  imageInputTokens?: number;
  imageOutputTokens?: number;
  avgLatencyMs: number;
  medianLatencyMs: number;
};

export type DashboardProviderStat = {
  provider: string;
  requests: number;
  totalTokens: number;
  costNanoUsd?: number;
  cachedTokens: number;
  uncachedInputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cacheWrite5mTokens?: number;
  cacheWrite1hTokens?: number;
  imageInputTokens?: number;
  imageOutputTokens?: number;
};

/** 按请求模型聚合的用量排行（客户端 model，空则 mapped_model）。 */
export type DashboardModelStat = {
  model: string;
  requests: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  costNanoUsd: number;
  cachedTokens: number;
  uncachedInputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cacheWrite5mTokens?: number;
  cacheWrite1hTokens?: number;
  imageInputTokens?: number;
  imageOutputTokens?: number;
};

export type DashboardUpstreamOption = {
  upstreamId: string;
  requests: number;
  totalTokens: number;
  cachedTokens: number;
  uncachedInputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cacheWrite5mTokens?: number;
  cacheWrite1hTokens?: number;
  imageInputTokens?: number;
  imageOutputTokens?: number;
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
  costNanoUsd?: number;
  cachedTokens: number;
  uncachedInputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  cacheWrite5mTokens?: number;
  cacheWrite1hTokens?: number;
  imageInputTokens?: number;
  imageOutputTokens?: number;
  totalTokens: number;
};

export type DashboardRequestItem = {
  id: number;
  tsMs: number;
  clientIp: string | null;
  path: string;
  provider: string;
  upstreamId: string;
  accountId?: string | null;
  model: string | null;
  mappedModel: string | null;
  /** 上游响应体里的 model。和实际发出的模型不同才在表格里标出来。 */
  upstreamResponseModel?: string | null;
  stream: boolean;
  status: number;
  totalTokens: number | null;
  outputTokens: number | null;
  cachedTokens: number | null;
  uncachedInputTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheWriteTokens?: number | null;
  cacheWrite5mTokens?: number | null;
  cacheWrite1hTokens?: number | null;
  imageInputTokens?: number | null;
  imageOutputTokens?: number | null;
  serviceTier?: string | null;
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
  models: DashboardModelStat[];
  /** 模型筛选选项（时间/渠道收窄，不受当前 model 筛选影响）。 */
  modelOptions: string[];
  upstreams: DashboardUpstreamOption[];
  series: DashboardSeriesPoint[];
  recent: DashboardRequestItem[];
  modelProbes: DashboardUpstreamModelProbe[];
  truncated: boolean;
};

export type DashboardSnapshotQuery = {
  range: DashboardRange;
  offset?: number;
  upstreamId?: string | null;
  /** 客户端请求模型 key（空 model 回退 mapped_model，再 (unknown)）。 */
  model?: string | null;
};
