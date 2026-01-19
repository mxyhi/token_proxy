import { m } from "@/paraglide/messages.js";

export const UPSTREAM_STRATEGIES = [
  { value: "priority_fill_first", label: () => m.upstream_strategy_priority_fill_first() },
  { value: "priority_round_robin", label: () => m.upstream_strategy_priority_round_robin() },
] as const;

export type UpstreamStrategy = (typeof UPSTREAM_STRATEGIES)[number]["value"];

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

export type UpstreamConfig = {
  id: string;
  provider: string;
  base_url: string;
  api_key: string | null;
  kiro_account_id?: string | null;
  preferred_endpoint?: KiroPreferredEndpoint | null;
  proxy_url: string | null;
  priority: number | null;
  enabled: boolean;
  model_mappings: Record<string, string>;
  overrides?: {
    header?: Record<string, string | null>;
  };
};

export type ProxyConfigFileBase = {
  host: string;
  port: number;
  local_api_key: string | null;
  app_proxy_url: string | null;
  kiro_preferred_endpoint?: KiroPreferredEndpoint | null;
  log_level?: LogLevel;
  tray_token_rate: TrayTokenRateConfig;
  enable_api_format_conversion: boolean;
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

export type ProxyServiceRequestState = "idle" | "working" | "error";

export type UpstreamForm = {
  id: string;
  provider: string;
  baseUrl: string;
  apiKey: string;
  kiroAccountId: string;
  preferredEndpoint: "" | KiroPreferredEndpoint;
  proxyUrl: string;
  priority: string;
  enabled: boolean;
  modelMappings: ModelMappingForm[];
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
  kiroPreferredEndpoint: "" | KiroPreferredEndpoint;
  logLevel: LogLevel;
  trayTokenRate: TrayTokenRateConfig;
  enableApiFormatConversion: boolean;
  upstreamStrategy: UpstreamStrategy;
  upstreams: UpstreamForm[];
};
