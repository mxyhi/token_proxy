export type RequestDetailCaptureState = {
  enabled: boolean;
  expiresAtMs: number | null;
};

/// 请求日志详情，包含表格展示的基础字段和详情面板的扩展字段
export type RequestLogDetail = {
  id: number;
  // 基础字段（与表格一致）
  tsMs: number;
  clientIp: string | null;
  path: string;
  provider: string;
  upstreamId: string;
  accountId?: string | null;
  model: string | null;
  mappedModel: string | null;
  upstreamResponseModel?: string | null;
  stream: boolean;
  status: number;
  inputTokens: number | null;
  outputTokens: number | null;
  uncachedInputTokens?: number | null;
  imageInputTokens?: number | null;
  imageOutputTokens: number | null;
  totalTokens: number | null;
  cachedTokens: number | null;
  cacheReadTokens?: number | null;
  cacheWriteTokens?: number | null;
  cacheWrite5mTokens?: number | null;
  cacheWrite1hTokens?: number | null;
  serviceTier?: string | null;
  costNanoUsd: number | null;
  pricingVersion: string | null;
  pricingModel: string | null;
  pricingContextTier: string | null;
  clientRequestId: string | null;
  attemptIndex: number | null;
  isBillable: boolean;
  latencyMs: number;
  upstreamFirstByteMs?: number | null;
  upstreamResponseHeadersMs?: number | null;
  upstreamFirstBodyChunkMs?: number | null;
  firstClientFlushMs?: number | null;
  firstOutputMs?: number | null;
  upstreamRequestId: string | null;
  // 详情扩展字段
  usageJson: string | null;
  requestHeaders: string | null;
  requestBody: string | null;
  responseBody: string | null;
  responseError: string | null;
};
