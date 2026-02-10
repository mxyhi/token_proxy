import {
  type ConfigForm,
  type InboundApiFormat,
  type KiroPreferredEndpoint,
  type ModelMappingForm,
  type ProxyConfigFile,
  type ProxyConfigFileBase,
  type TrayTokenRateConfig,
  type UpstreamForm,
  TRAY_TOKEN_RATE_FORMATS,
} from "@/features/config/types";
import { createNativeInboundFormatSet, removeInboundFormatsInSet } from "@/features/config/inbound-formats";
import { m } from "@/paraglide/messages.js";

const DEFAULT_TRAY_TOKEN_RATE: TrayTokenRateConfig = {
  enabled: true,
  format: "split",
};

const INTEGER_PATTERN = /^-?\d+$/;
let modelMappingCounter = 0;

const TRAY_TOKEN_RATE_FORMAT_VALUES: ReadonlySet<string> = new Set(
  TRAY_TOKEN_RATE_FORMATS.map((format) => format.value)
);

function isKiroPreferredEndpoint(value: string): value is KiroPreferredEndpoint {
  return value === "ide" || value === "cli";
}

function normalizeKiroPreferredEndpoint(value: string) {
  const trimmed = value.trim();
  if (isKiroPreferredEndpoint(trimmed)) {
    return trimmed;
  }
  return null;
}

function joinListInput(values: string[] | null | undefined) {
  return values && values.length ? values.join(", ") : "";
}

function parseListInput(value: string) {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

const KNOWN_CONFIG_KEYS: ReadonlySet<string> = new Set([
  "host",
  "port",
  "local_api_key",
  "app_proxy_url",
  "kiro_preferred_endpoint",
  "antigravity_ide_db_path",
  "antigravity_app_paths",
  "antigravity_process_names",
  "antigravity_user_agent",
  "log_level",
  "tray_token_rate",
  "upstream_strategy",
  "upstreams",
]);

export const EMPTY_FORM: ConfigForm = {
  host: "127.0.0.1",
  port: "9208",
  localApiKey: "",
  appProxyUrl: "",
  kiroPreferredEndpoint: "ide",
  antigravityIdeDbPath: "",
  antigravityAppPaths: "",
  antigravityProcessNames: "",
  antigravityUserAgent: "",
  logLevel: "silent",
  trayTokenRate: { ...DEFAULT_TRAY_TOKEN_RATE },
  upstreamStrategy: "priority_fill_first",
  upstreams: [],
};

export function createEmptyUpstream(): UpstreamForm {
  return {
    id: "",
    providers: ["openai"],
    baseUrl: "",
    apiKey: "",
    filterPromptCacheRetention: false,
    filterSafetyIdentifier: false,
    kiroAccountId: "",
    codexAccountId: "",
    antigravityAccountId: "",
    preferredEndpoint: "",
    proxyUrl: "",
    priority: "",
    // 新增上游默认作为草稿，避免用户尚未填完必填项时被“无法保存”阻塞。
    enabled: false,
    modelMappings: [],
    convertFromMap: {},
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
    kiroPreferredEndpoint: config.kiro_preferred_endpoint ?? "ide",
    antigravityIdeDbPath: config.antigravity_ide_db_path ?? "",
    antigravityAppPaths: joinListInput(config.antigravity_app_paths),
    antigravityProcessNames: joinListInput(config.antigravity_process_names),
    antigravityUserAgent: config.antigravity_user_agent ?? "",
    logLevel: config.log_level ?? "silent",
    trayTokenRate: normalizeTrayTokenRate(config.tray_token_rate),
    upstreamStrategy: config.upstream_strategy,
    upstreams: config.upstreams.map((upstream) => ({
      id: upstream.id,
      providers: upstream.providers ?? [],
      baseUrl: upstream.base_url,
      apiKey: upstream.api_key ?? "",
      filterPromptCacheRetention: upstream.filter_prompt_cache_retention ?? false,
      filterSafetyIdentifier: upstream.filter_safety_identifier ?? false,
      kiroAccountId: upstream.kiro_account_id ?? "",
      codexAccountId: upstream.codex_account_id ?? "",
      antigravityAccountId: upstream.antigravity_account_id ?? "",
      preferredEndpoint: upstream.preferred_endpoint ?? "",
      proxyUrl: upstream.proxy_url ?? "",
      priority: upstream.priority === null ? "" : String(upstream.priority),
      enabled: upstream.enabled,
      modelMappings: toModelMappingForm(upstream.model_mappings),
      convertFromMap: upstream.convert_from_map ?? {},
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
    kiro_preferred_endpoint: normalizeKiroPreferredEndpoint(form.kiroPreferredEndpoint),
    antigravity_ide_db_path: form.antigravityIdeDbPath.trim()
      ? form.antigravityIdeDbPath.trim()
      : null,
    antigravity_app_paths: parseListInput(form.antigravityAppPaths),
    antigravity_process_names: parseListInput(form.antigravityProcessNames),
    antigravity_user_agent: form.antigravityUserAgent.trim()
      ? form.antigravityUserAgent.trim()
      : null,
    log_level: form.logLevel,
    tray_token_rate: form.trayTokenRate,
    upstream_strategy: form.upstreamStrategy,
    upstreams: form.upstreams.map((upstream) => {
      const providers = normalizeProviders(upstream.providers);
      return {
        id: upstream.id.trim(),
        providers,
        base_url: upstream.baseUrl.trim(),
        api_key: upstream.apiKey.trim() ? upstream.apiKey.trim() : null,
        filter_prompt_cache_retention: upstream.filterPromptCacheRetention,
        filter_safety_identifier: upstream.filterSafetyIdentifier,
        kiro_account_id: upstream.kiroAccountId.trim()
          ? upstream.kiroAccountId.trim()
          : null,
        codex_account_id: upstream.codexAccountId.trim()
          ? upstream.codexAccountId.trim()
          : null,
        antigravity_account_id: upstream.antigravityAccountId.trim()
          ? upstream.antigravityAccountId.trim()
          : null,
        preferred_endpoint: normalizeKiroPreferredEndpoint(upstream.preferredEndpoint),
        proxy_url: upstream.proxyUrl.trim() ? upstream.proxyUrl.trim() : null,
        priority: parseOptionalInt(upstream.priority),
        enabled: upstream.enabled,
        model_mappings: toModelMappingPayload(upstream.modelMappings),
        convert_from_map: normalizeConvertFromMap(upstream.convertFromMap, providers),
        overrides: toOverridesPayload(upstream.overrides),
      };
    }),
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

    // 允许上游为空 / 全部禁用：仅对启用中的上游做强校验；禁用项保留为“草稿”。
    if (!upstream.enabled) {
      continue;
    }

    const providers = normalizeProviders(upstream.providers);
    if (!providers.length) {
      return { valid: false, message: m.error_upstream_provider_required({ id }) };
    }
    const specialProviders = providers.filter((provider) =>
      provider === "kiro" || provider === "codex" || provider === "antigravity",
    );
    if (specialProviders.length && providers.length > 1) {
      return {
        valid: false,
        message: m.error_upstream_provider_required({ id }),
      };
    }
    if (providers.includes("kiro") && !upstream.kiroAccountId.trim()) {
      return { valid: false, message: m.error_upstream_kiro_account_required({ id }) };
    }
    if (providers.includes("codex") && !upstream.codexAccountId.trim()) {
      return { valid: false, message: m.error_upstream_codex_account_required({ id }) };
    }
    if (providers.includes("antigravity") && !upstream.antigravityAccountId.trim()) {
      return { valid: false, message: m.error_upstream_antigravity_account_required({ id }) };
    }

    const canOmitBaseUrl =
      providers.length === 1 &&
      (providers[0] === "kiro" || providers[0] === "codex" || providers[0] === "antigravity");
    if (!canOmitBaseUrl && !upstream.baseUrl.trim()) {
      return { valid: false, message: m.error_upstream_base_url_required({ id }) };
    }

    const convertFromMapProviders = Object.keys(upstream.convertFromMap);
    for (const provider of convertFromMapProviders) {
      if (!providers.includes(provider)) {
        return { valid: false, message: m.error_upstream_provider_required({ id }) };
      }
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

function normalizeProviders(values: readonly string[]) {
  const seen = new Set<string>();
  const output: string[] = [];
  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed) {
      continue;
    }
    if (seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    output.push(trimmed);
  }
  return output;
}

function normalizeConvertFromMap(
  map: Record<string, InboundApiFormat[]>,
  providers: readonly string[],
) {
  if (!Object.keys(map).length) {
    return undefined;
  }
  const providerSet = new Set(providers);
  // 若某个入站格式已经被该 upstream 的某个 provider 原生支持，则无需（也不应）再通过 convert_from_map
  // 把它授权给其它 provider，否则只会造成冗余与误导。
  const nativeFormatsInUpstream = createNativeInboundFormatSet(providers);
  const outputEntries: Array<[string, InboundApiFormat[]]> = [];
  for (const [provider, formats] of Object.entries(map)) {
    if (!providerSet.has(provider)) {
      continue;
    }
    // UI 会隐藏该 upstream 已原生支持的入站格式；这里也做一次清理，避免历史冗余配置被保存回去。
    const filtered = removeInboundFormatsInSet(formats, nativeFormatsInUpstream);
    const unique: InboundApiFormat[] = [];
    const seen = new Set<InboundApiFormat>();
    for (const format of filtered) {
      if (seen.has(format)) {
        continue;
      }
      seen.add(format);
      unique.push(format);
    }
    if (!unique.length) {
      continue;
    }
    outputEntries.push([provider, unique]);
  }
  if (!outputEntries.length) {
    return undefined;
  }
  return Object.fromEntries(outputEntries);
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
