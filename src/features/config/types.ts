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

/** 账户型 credential 的 provider；与后端 AccountProvider 对齐。 */
export type AccountProviderKind = "kiro" | "codex" | "xai";

/**
 * Upstream 唯一凭据形态（与后端 UpstreamCredential 对齐）。
 * 禁止再平铺 api_keys + 多个可选 *_account_id。
 */
export type UpstreamCredentialConfig =
  | { type: "api_keys"; api_keys?: string[] }
  | { type: "account"; provider: AccountProviderKind; account_id: string }
  | { type: "passthrough" };

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
  /** 判别联合凭据；账户型 / API Key / 透传互斥。 */
  credential?: UpstreamCredentialConfig;
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
  preferred_endpoint?: KiroPreferredEndpoint | null;
  proxy_url: string | null;
  priority: number | null;
  enabled: boolean;
  /** Empty or missing means this upstream accepts every inbound model id. */
  available_models?: string[];
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
  cors_enabled?: boolean;
  model_list_prefix?: boolean;
  /** 后端一次性迁移标记：旧配置已默认打开 model_list_prefix */
  model_list_prefix_default_on_migrated?: boolean;
  kiro_preferred_endpoint?: KiroPreferredEndpoint | null;
  log_level?: LogLevel;
  retryable_failure_cooldown_secs?: number;
  same_upstream_retry_count?: number;
  codex_session_scoped_cooldown_enabled?: boolean;
  xai_inject_x_search?: boolean;
  stream_first_output_timeout_secs?: number;
  sync_response_timeout_secs?: number;
  tray_token_rate: TrayTokenRateConfig;
  upstream_strategy: UpstreamStrategy;
  hot_model_mappings?: Record<string, string>;
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

export type AgentNodeConfig = {
  enabled: boolean;
  server_url: string;
  api_key: string;
  hostname: string | null;
};

export type AgentNodeServiceState = "running" | "stopped";

export type AgentNodeServiceStatus = {
  state: AgentNodeServiceState;
  enabled: boolean;
  server_url: string | null;
  hostname: string | null;
  last_error: string | null;
  started_at_ms: number | null;
};

export type AgentNodeRequestState = "idle" | "working" | "error";

export type UpstreamForm = {
  id: string;
  providers: string[];
  baseUrl: string;
  apiKeys: string;
  filterPromptCacheRetention: boolean;
  filterSafetyIdentifier: boolean;
  useChatCompletionsForResponses: boolean;
  rewriteDeveloperRoleToSystem: boolean;
  /**
   * 账户型 credential 的 account_id（与 providers 中唯一账户 provider 绑定）。
   * 非账户型上游保持空字符串。
   */
  accountId: string;
  preferredEndpoint: "" | KiroPreferredEndpoint;
  proxyUrl: string;
  priority: string;
  enabled: boolean;
  availableModelsMode: "all" | "selected";
  availableModels: string[];
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
  corsEnabled: boolean;
  modelListPrefix: boolean;
  kiroPreferredEndpoint: "" | KiroPreferredEndpoint;
  logLevel: LogLevel;
  retryableFailureCooldownSecs: string;
  sameUpstreamRetryCount: string;
  codexSessionScopedCooldownEnabled: boolean;
  xaiInjectXSearch: boolean;
  streamFirstOutputTimeoutSecs: string;
  syncResponseTimeoutSecs: string;
  trayTokenRate: TrayTokenRateConfig;
  upstreamStrategy: {
    order: UpstreamOrderStrategy;
    dispatchType: UpstreamDispatchType;
    hedgeDelayMs: string;
    maxParallel: string;
  };
  hotModelMappings: ModelMappingForm[];
  upstreams: UpstreamForm[];
};
