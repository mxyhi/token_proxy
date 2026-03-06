export type RequestDetailCaptureState = {
  enabled: boolean;
  expiresAtMs: number | null;
};

/// 请求日志详情，包含表格展示的基础字段和详情面板的扩展字段
export type RequestLogDetail = {
  id: number;
  // 基础字段（与表格一致）
  tsMs: number;
  path: string;
  provider: string;
  upstreamId: string;
  model: string | null;
  mappedModel: string | null;
  stream: boolean;
  status: number;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  cachedTokens: number | null;
  latencyMs: number;
  upstreamRequestId: string | null;
  // 详情扩展字段
  usageJson: string | null;
  requestHeaders: string | null;
  requestBody: string | null;
  responseError: string | null;
};
