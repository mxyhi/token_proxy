import {
  type ConfigForm,
  type ModelMappingForm,
  type ProxyConfigFile,
  type ProxyConfigFileBase,
  type TrayTokenRateConfig,
  type UpstreamForm,
  TRAY_TOKEN_RATE_FORMATS,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

const DEFAULT_UPSTREAMS: UpstreamForm[] = [
  {
    id: "openai-default",
    provider: "openai",
    baseUrl: "https://api.openai.com",
    apiKey: "",
    proxyUrl: "",
    priority: "0",
    enabled: true,
    modelMappings: [],
    overrides: { header: [] },
  },
  {
    id: "openai-responses",
    provider: "openai-response",
    baseUrl: "https://api.openai.com",
    apiKey: "",
    proxyUrl: "",
    priority: "0",
    enabled: true,
    modelMappings: [],
    overrides: { header: [] },
  },
  {
    id: "anthropic-default",
    provider: "anthropic",
    baseUrl: "https://api.anthropic.com",
    apiKey: "",
    proxyUrl: "",
    priority: "0",
    enabled: true,
    modelMappings: [],
    overrides: { header: [] },
  },
  {
    id: "gemini-default",
    provider: "gemini",
    baseUrl: "https://generativelanguage.googleapis.com",
    apiKey: "",
    proxyUrl: "",
    priority: "0",
    enabled: true,
    modelMappings: [],
    overrides: { header: [] },
  },
];

const DEFAULT_TRAY_TOKEN_RATE: TrayTokenRateConfig = {
  enabled: true,
  format: "split",
};

const INTEGER_PATTERN = /^-?\d+$/;
let modelMappingCounter = 0;

const TRAY_TOKEN_RATE_FORMAT_VALUES: ReadonlySet<string> = new Set(
  TRAY_TOKEN_RATE_FORMATS.map((format) => format.value)
);

const KNOWN_CONFIG_KEYS: ReadonlySet<string> = new Set([
  "host",
  "port",
  "local_api_key",
  "app_proxy_url",
  "log_path",
  "log_level",
  "tray_token_rate",
  "enable_api_format_conversion",
  "upstream_strategy",
  "upstreams",
]);

export const EMPTY_FORM: ConfigForm = {
  host: "127.0.0.1",
  port: "9208",
  localApiKey: "",
  appProxyUrl: "",
  logPath: "proxy.log",
  logLevel: "silent",
  trayTokenRate: { ...DEFAULT_TRAY_TOKEN_RATE },
  enableApiFormatConversion: false,
  upstreamStrategy: "priority_fill_first",
  upstreams: DEFAULT_UPSTREAMS.map((upstream) => ({ ...upstream })),
};

export function createEmptyUpstream(): UpstreamForm {
  return {
    id: "",
    provider: "",
    baseUrl: "",
    apiKey: "",
    proxyUrl: "",
    priority: "",
    enabled: true,
    modelMappings: [],
    overrides: { header: [] },
  };
}

export function createModelMapping(pattern = "", target = "") {
  // 稳定 id 用于列表渲染，避免输入时因 key 变化导致焦点丢失
  modelMappingCounter += 1;
  return {
    id: `model-mapping-${Date.now()}-${modelMappingCounter}`,
    pattern,
    target,
  };
}

export function extractConfigExtras(config: ProxyConfigFile) {
  const extras: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(config)) {
    if (!KNOWN_CONFIG_KEYS.has(key)) {
      extras[key] = value;
    }
  }
  return extras;
}

export function mergeConfigExtras(
  config: ProxyConfigFileBase,
  extras: Record<string, unknown>
) {
  return {
    ...extras,
    ...config,
  };
}

export function toForm(config: ProxyConfigFile): ConfigForm {
  return {
    host: config.host,
    port: String(config.port),
    localApiKey: config.local_api_key ?? "",
    appProxyUrl: config.app_proxy_url ?? "",
    logPath: config.log_path,
    logLevel: config.log_level ?? "silent",
    trayTokenRate: normalizeTrayTokenRate(config.tray_token_rate),
    enableApiFormatConversion: config.enable_api_format_conversion,
    upstreamStrategy: config.upstream_strategy,
    upstreams: config.upstreams.map((upstream) => ({
      id: upstream.id,
      provider: upstream.provider,
      baseUrl: upstream.base_url,
      apiKey: upstream.api_key ?? "",
      proxyUrl: upstream.proxy_url ?? "",
      priority: upstream.priority === null ? "" : String(upstream.priority),
      enabled: upstream.enabled,
      modelMappings: toModelMappingForm(upstream.model_mappings),
      overrides: normalizeOverrides(upstream.overrides),
    })),
  };
}

export function toPayload(form: ConfigForm): ProxyConfigFile {
  const port = Number.parseInt(form.port, 10);
  return {
    host: form.host.trim(),
    port,
    local_api_key: form.localApiKey.trim() ? form.localApiKey.trim() : null,
    app_proxy_url: form.appProxyUrl.trim() ? form.appProxyUrl.trim() : null,
    log_path: form.logPath.trim(),
    log_level: form.logLevel,
    tray_token_rate: form.trayTokenRate,
    enable_api_format_conversion: form.enableApiFormatConversion,
    upstream_strategy: form.upstreamStrategy,
    upstreams: form.upstreams.map((upstream) => ({
      id: upstream.id.trim(),
      provider: upstream.provider.trim(),
      base_url: upstream.baseUrl.trim(),
      api_key: upstream.apiKey.trim() ? upstream.apiKey.trim() : null,
      proxy_url: upstream.proxyUrl.trim() ? upstream.proxyUrl.trim() : null,
      priority: parseOptionalInt(upstream.priority),
      enabled: upstream.enabled,
      model_mappings: toModelMappingPayload(upstream.modelMappings),
      overrides: toOverridesPayload(upstream.overrides),
    })),
  };
}

export function validate(form: ConfigForm) {
  if (!form.host.trim()) {
    return { valid: false, message: m.error_host_required() };
  }
  const port = Number.parseInt(form.port, 10);
  if (!Number.isFinite(port) || port < 1 || port > 65535) {
    return { valid: false, message: m.error_port_range() };
  }
  if (form.appProxyUrl.trim() && !isValidProxyUrl(form.appProxyUrl.trim())) {
    return { valid: false, message: m.error_app_proxy_url_invalid() };
  }
  if (!form.logPath.trim()) {
    return { valid: false, message: m.error_log_path_required() };
  }
  if (!form.upstreams.length) {
    return { valid: false, message: m.error_upstream_at_least_one() };
  }
  const hasEnabledUpstream = form.upstreams.some((upstream) => upstream.enabled);
  if (!hasEnabledUpstream) {
    return { valid: false, message: m.error_upstream_at_least_one_enabled() };
  }

  const ids = new Set<string>();
  for (const upstream of form.upstreams) {
    const id = upstream.id.trim();
    if (!id) {
      return { valid: false, message: m.error_upstream_id_required() };
    }
    if (ids.has(id)) {
      return { valid: false, message: m.error_upstream_id_unique({ id }) };
    }
    ids.add(id);
    const provider = upstream.provider.trim();
    if (!provider) {
      return { valid: false, message: m.error_upstream_provider_required({ id }) };
    }
    if (!upstream.baseUrl.trim()) {
      return { valid: false, message: m.error_upstream_base_url_required({ id }) };
    }
    const upstreamProxyUrl = upstream.proxyUrl.trim();
    if (upstreamProxyUrl) {
      if (upstreamProxyUrl === APP_PROXY_URL_PLACEHOLDER) {
        if (!form.appProxyUrl.trim()) {
          return { valid: false, message: m.error_upstream_proxy_url_requires_app({ id }) };
        }
      } else if (!isValidProxyUrl(upstreamProxyUrl)) {
        return { valid: false, message: m.error_upstream_proxy_url_invalid({ id }) };
      }
    }
    if (!isValidOptionalInt(upstream.priority)) {
      return { valid: false, message: m.error_upstream_priority_integer({ id }) };
    }
    const mappingError = validateModelMappings(upstream.modelMappings, id);
    if (mappingError) {
      return { valid: false, message: mappingError };
    }
    const headerOverrideError = validateHeaderOverrides(upstream.overrides.header, id);
    if (headerOverrideError) {
      return { valid: false, message: headerOverrideError };
    }
  }

  return { valid: true, message: "" };
}

const APP_PROXY_URL_PLACEHOLDER = "$app_proxy_url";

const PROXY_URL_PROTOCOLS: ReadonlySet<string> = new Set([
  "http:",
  "https:",
  "socks5:",
  "socks5h:",
]);

function isValidProxyUrl(value: string) {
  try {
    const parsed = new URL(value);
    return PROXY_URL_PROTOCOLS.has(parsed.protocol);
  } catch (_) {
    return false;
  }
}

function toModelMappingForm(mappings: Record<string, string>): ModelMappingForm[] {
  return Object.entries(mappings).map(([pattern, target]) =>
    createModelMapping(pattern, target),
  );
}

type HeaderOverrideForm = ConfigForm["upstreams"][number]["overrides"]["header"][number];

function normalizeOverrides(
  overrides?: ProxyConfigFile["upstreams"][number]["overrides"],
): ConfigForm["upstreams"][number]["overrides"] {
  const header = Object.entries(overrides?.header ?? {}).map(
    ([name, value], index): HeaderOverrideForm => ({
      id: `header-override-${Date.now()}-${index}`,
      name,
      value: value ?? "",
      isNull: value === null,
    }),
  );
  return { header };
}

function toModelMappingPayload(mappings: ModelMappingForm[]) {
  const entries = mappings.map(
    (mapping): [string, string] => [mapping.pattern.trim(), mapping.target.trim()],
  );
  return Object.fromEntries(entries);
}

function toOverridesPayload(
  overrides: ConfigForm["upstreams"][number]["overrides"],
) {
  const headerEntries = overrides.header
    .map(({ name, value, isNull }) => [name.trim(), isNull ? null : value.trim()] as const)
    .filter(([name]) => name);
  if (!headerEntries.length) {
    return undefined;
  }
  return {
    header: Object.fromEntries(headerEntries),
  };
}

function normalizeTrayTokenRate(value: TrayTokenRateConfig) {
  if (!TRAY_TOKEN_RATE_FORMAT_VALUES.has(value.format)) {
    return { ...value, format: DEFAULT_TRAY_TOKEN_RATE.format };
  }
  return value;
}

function validateModelMappings(mappings: ModelMappingForm[], upstreamId: string) {
  const seen = new Set<string>();
  let wildcardSeen = false;
  for (let index = 0; index < mappings.length; index += 1) {
    const row = String(index + 1);
    const pattern = mappings[index]?.pattern.trim() ?? "";
    const target = mappings[index]?.target.trim() ?? "";
    if (!pattern) {
      return m.error_model_mapping_pattern_required({ id: upstreamId, row });
    }
    if (!target) {
      return m.error_model_mapping_target_required({ id: upstreamId, row });
    }
    if (seen.has(pattern)) {
      return m.error_model_mapping_pattern_duplicate({ id: upstreamId, pattern });
    }
    seen.add(pattern);

    if (pattern === "*") {
      if (wildcardSeen) {
        return m.error_model_mapping_wildcard_multiple({ id: upstreamId });
      }
      wildcardSeen = true;
      continue;
    }

    if (pattern.includes("*") && !pattern.endsWith("*")) {
      return m.error_model_mapping_pattern_invalid({ id: upstreamId, pattern });
    }

    if (pattern.endsWith("*")) {
      const prefix = pattern.slice(0, -1).trim();
      if (!prefix) {
        return m.error_model_mapping_prefix_required({ id: upstreamId, row });
      }
      if (prefix.includes("*")) {
        return m.error_model_mapping_pattern_invalid({ id: upstreamId, pattern });
      }
    }
  }
  return "";
}

function validateHeaderOverrides(
  overrides: ConfigForm["upstreams"][number]["overrides"]["header"],
  upstreamId: string
) {
  for (let index = 0; index < overrides.length; index += 1) {
    const row = String(index + 1);
    const name = overrides[index]?.name.trim() ?? "";
    if (!name) {
      return m.error_header_override_name_required({ id: upstreamId, row });
    }
    const isValid = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name);
    if (!isValid) {
      return m.error_header_override_name_invalid({ id: upstreamId, row });
    }
  }
  return "";
}

function isValidOptionalInt(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return true;
  }
  return INTEGER_PATTERN.test(trimmed);
}

function parseOptionalInt(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  if (!INTEGER_PATTERN.test(trimmed)) {
    return null;
  }
  const number = Number.parseInt(trimmed, 10);
  return Number.isFinite(number) ? number : null;
}
