import { m } from "@/paraglide/messages.js";

export const UPSTREAM_ORDER_STRATEGIES = [
  { value: "fill_first", label: () => m.upstream_strategy_order_fill_first() },
  { value: "round_robin", label: () => m.upstream_strategy_order_round_robin() },
] as const;

export type UpstreamOrderStrategy = (typeof UPSTREAM_ORDER_STRATEGIES)[number]["value"];

export const UPSTREAM_DISPATCH_STRATEGIES = [
  { value: "serial", label: () => m.upstream_strategy_dispatch_serial() },
  { value: "hedged", label: () => m.upstream_strategy_dispatch_hedged() },
  { value: "race", label: () => m.upstream_strategy_dispatch_race() },
] as const;

export type UpstreamDispatchType = (typeof UPSTREAM_DISPATCH_STRATEGIES)[number]["value"];

export type UpstreamDispatchStrategy =
  | { type: "serial" }
  | { type: "hedged"; delay_ms: number; max_parallel: number }
  | { type: "race"; max_parallel: number };

export type UpstreamStrategy = {
  order: UpstreamOrderStrategy;
  dispatch: UpstreamDispatchStrategy;
};

export const TRAY_TOKEN_RATE_FORMATS = [
  { value: "combined", label: () => m.proxy_core_tray_token_rate_format_combined() },
  { value: "split", label: () => m.proxy_core_tray_token_rate_format_split() },
  { value: "both", label: () => m.proxy_core_tray_token_rate_format_both() },
] as const;

export type TrayTokenRateFormat = (typeof TRAY_TOKEN_RATE_FORMATS)[number]["value"];

export type KiroPreferredEndpoint = "ide" | "cli";

export type LogLevel = "silent" | "error" | "warn" | "info" | "debug" | "trace";

export type TrayTokenRateConfig = {
  enabled: boolean;
  format: TrayTokenRateFormat;
};

export type InboundApiFormat =
  | "openai_chat"
  | "openai_responses"
  | "anthropic_messages"
  | "gemini";

export type UpstreamConfig = {
  id: string;
  /**
   * 一个 upstream 可以同时声明多个 provider（同一条 base_url/api_keys 复用）。
   *
   * 说明：后端会把它展开为“每个 provider × 每个 api key 一条运行时 upstream”，
   * 并按 provider 维度做负载均衡。
   */
  providers?: string[];
  base_url: string;
  api_keys?: string[];
  /**
   * Whether to drop OpenAI Responses request field `prompt_cache_retention` before sending upstream.
   *
   * Only meaningful for provider "openai-response".
   */
  filter_prompt_cache_retention?: boolean;
  /**
   * Whether to drop OpenAI Responses request field `safety_identifier` before sending upstream.
   *
   * Only meaningful for provider "openai-response".
   */
  filter_safety_identifier?: boolean;
  /**
   * Whether to send inbound `/v1/responses` requests to `/v1/chat/completions` for this upstream.
   *
   * Only meaningful for provider "openai-response".
   */
  use_chat_completions_for_responses?: boolean;
  /**
   * Whether to rewrite OpenAI-compatible role `developer` to `system` before sending upstream.
   */
  rewrite_developer_role_to_system?: boolean;
  kiro_account_id?: string | null;
  codex_account_id?: string | null;
  preferred_endpoint?: KiroPreferredEndpoint | null;
  proxy_url: string | null;
  priority: number | null;
  enabled: boolean;
  model_mappings: Record<string, string>;
  /**
   * 允许从哪些“入站 API 格式”转换后再使用该 provider。
   * key 必须在 `providers[]` 内。
   *
   * - 为空/缺失：仅允许该 provider 的 native 格式（更安全、可控）
   * - 非空：允许跨格式 fallback（例如 /v1/messages → openai-response）
   */
  convert_from_map?: Record<string, InboundApiFormat[]>;
  overrides?: {
    header?: Record<string, string | null>;
  };
};

export type ProxyConfigFileBase = {
  host: string;
  port: number;
  local_api_key: string | null;
  app_proxy_url: string | null;
  model_list_prefix?: boolean;
  kiro_preferred_endpoint?: KiroPreferredEndpoint | null;
  log_level?: LogLevel;
  retryable_failure_cooldown_secs?: number;
  upstream_no_data_timeout_secs?: number;
  tray_token_rate: TrayTokenRateConfig;
  upstream_strategy: UpstreamStrategy;
  upstreams: UpstreamConfig[];
};

export type ProxyConfigFile = ProxyConfigFileBase & Record<string, unknown>;

export type ConfigResponse = {
  path: string;
  config: ProxyConfigFile;
};

export type ProxyServiceState = "running" | "stopped";

export type ProxyServiceStatus = {
  state: ProxyServiceState;
  addr: string | null;
  last_error: string | null;
};

export type SaveProxyConfigResult = {
  status: ProxyServiceStatus;
  apply_error: string | null;
};

export type ProxyServiceRequestState = "idle" | "working" | "error";

export type UpstreamForm = {
  id: string;
  providers: string[];
  baseUrl: string;
  apiKeys: string;
  filterPromptCacheRetention: boolean;
  filterSafetyIdentifier: boolean;
  useChatCompletionsForResponses: boolean;
  rewriteDeveloperRoleToSystem: boolean;
  preferredEndpoint: "" | KiroPreferredEndpoint;
  proxyUrl: string;
  priority: string;
  enabled: boolean;
  modelMappings: ModelMappingForm[];
  convertFromMap: Record<string, InboundApiFormat[]>;
  overrides: {
    header: HeaderOverrideForm[];
  };
};

export type HeaderOverrideForm = {
  id: string;
  name: string;
  value: string;
  isNull: boolean;
};

export type ModelMappingForm = {
  id: string;
  pattern: string;
  target: string;
};

export type ConfigForm = {
  host: string;
  port: string;
  localApiKey: string;
  appProxyUrl: string;
  modelListPrefix: boolean;
  kiroPreferredEndpoint: "" | KiroPreferredEndpoint;
  logLevel: LogLevel;
  retryableFailureCooldownSecs: string;
  upstreamNoDataTimeoutSecs: string;
  trayTokenRate: TrayTokenRateConfig;
  upstreamStrategy: {
    order: UpstreamOrderStrategy;
    dispatchType: UpstreamDispatchType;
    hedgeDelayMs: string;
    maxParallel: string;
  };
  upstreams: UpstreamForm[];
};
