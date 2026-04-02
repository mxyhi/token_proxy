import {
  type ConfigForm,
  type InboundApiFormat,
  type KiroPreferredEndpoint,
  type ModelMappingForm,
  type ProxyConfigFile,
  type ProxyConfigFileBase,
  type TrayTokenRateConfig,
  type UpstreamDispatchStrategy,
  type UpstreamForm,
  type UpstreamStrategy,
  TRAY_TOKEN_RATE_FORMATS,
} from "@/features/config/types";
import { createNativeInboundFormatSet, removeInboundFormatsInSet } from "@/features/config/inbound-formats";
import { m } from "@/paraglide/messages.js";

const DEFAULT_TRAY_TOKEN_RATE: TrayTokenRateConfig = {
  enabled: true,
  format: "split",
};

const MIN_UPSTREAM_NO_DATA_TIMEOUT_SECS = 3;
const DEFAULT_UPSTREAM_NO_DATA_TIMEOUT_SECS = 120;
const DEFAULT_HEDGE_DELAY_MS = 2000;
const DEFAULT_MAX_PARALLEL = 2;
const MIN_PARALLEL_ATTEMPTS = 2;
const SUPPORTED_PROVIDERS = new Set([
  "openai",
  "openai-response",
  "anthropic",
  "gemini",
  "kiro",
  "codex",
]);
const INTEGER_PATTERN = /^-?\d+$/;
const NON_NEGATIVE_INTEGER_PATTERN = /^\d+$/;
const POSITIVE_INTEGER_PATTERN = /^[1-9]\d*$/;
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

function isAccountBackedProviderSet(providers: readonly string[]) {
  return (
    providers.length === 1 &&
    (providers[0] === "kiro" || providers[0] === "codex")
  );
}

function joinListInput(values: string[] | null | undefined) {
  return values && values.length ? values.join(", ") : "";
}

function parseApiKeysInput(value: string) {
  const seen = new Set<string>();
  const output: string[] = [];
  for (const item of value.split(/[,\n]/)) {
    const trimmed = item.trim();
    if (!trimmed || seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    output.push(trimmed);
  }
  return output;
}

const KNOWN_CONFIG_KEYS: ReadonlySet<string> = new Set([
  "host",
  "port",
  "local_api_key",
  "app_proxy_url",
  "model_list_prefix",
  "kiro_preferred_endpoint",
  "log_level",
  "retryable_failure_cooldown_secs",
  "upstream_no_data_timeout_secs",
  "tray_token_rate",
  "upstream_strategy",
  "upstreams",
]);

export const EMPTY_FORM: ConfigForm = {
  host: "127.0.0.1",
  port: "9208",
  localApiKey: "",
  appProxyUrl: "",
  modelListPrefix: false,
  kiroPreferredEndpoint: "ide",
  logLevel: "silent",
  retryableFailureCooldownSecs: "15",
  upstreamNoDataTimeoutSecs: String(DEFAULT_UPSTREAM_NO_DATA_TIMEOUT_SECS),
  trayTokenRate: { ...DEFAULT_TRAY_TOKEN_RATE },
  upstreamStrategy: {
    order: "fill_first",
    dispatchType: "serial",
    hedgeDelayMs: String(DEFAULT_HEDGE_DELAY_MS),
    maxParallel: String(DEFAULT_MAX_PARALLEL),
  },
  upstreams: [],
};

export function createEmptyUpstream(): UpstreamForm {
  return {
    id: "",
    providers: ["openai"],
    baseUrl: "",
    apiKeys: "",
    filterPromptCacheRetention: false,
    filterSafetyIdentifier: false,
    useChatCompletionsForResponses: false,
    rewriteDeveloperRoleToSystem: false,
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

function createAccountBackedUpstream(provider: "kiro" | "codex"): UpstreamForm {
  return {
    ...createEmptyUpstream(),
    id: `${provider}-default`,
    providers: [provider],
    enabled: true,
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
    modelListPrefix: config.model_list_prefix ?? false,
    kiroPreferredEndpoint: config.kiro_preferred_endpoint ?? "ide",
    logLevel: config.log_level ?? "silent",
    retryableFailureCooldownSecs: String(config.retryable_failure_cooldown_secs ?? 15),
    upstreamNoDataTimeoutSecs: String(
      config.upstream_no_data_timeout_secs ?? DEFAULT_UPSTREAM_NO_DATA_TIMEOUT_SECS,
    ),
    trayTokenRate: normalizeTrayTokenRate(config.tray_token_rate),
    upstreamStrategy: toUpstreamStrategyForm(config.upstream_strategy),
    upstreams: config.upstreams.map((upstream) => {
      const providers = upstream.providers ?? [];
      const omitNetworkFields = isAccountBackedProviderSet(providers);
      return {
        id: upstream.id,
        providers,
        baseUrl: omitNetworkFields ? "" : upstream.base_url,
        apiKeys: joinListInput(upstream.api_keys),
        filterPromptCacheRetention: upstream.filter_prompt_cache_retention ?? false,
        filterSafetyIdentifier: upstream.filter_safety_identifier ?? false,
        useChatCompletionsForResponses: upstream.use_chat_completions_for_responses ?? false,
        rewriteDeveloperRoleToSystem: upstream.rewrite_developer_role_to_system ?? false,
        preferredEndpoint: upstream.preferred_endpoint ?? "",
        proxyUrl: omitNetworkFields ? "" : upstream.proxy_url ?? "",
        priority: upstream.priority === null ? "" : String(upstream.priority),
        enabled: upstream.enabled,
        modelMappings: toModelMappingForm(upstream.model_mappings),
        convertFromMap: upstream.convert_from_map ?? {},
        overrides: normalizeOverrides(upstream.overrides),
      };
    }),
  };
}

export function toPayload(form: ConfigForm): ProxyConfigFile {
  const port = Number.parseInt(form.port, 10);
  return {
    host: form.host.trim(),
    port,
    local_api_key: form.localApiKey.trim() ? form.localApiKey.trim() : null,
    app_proxy_url: form.appProxyUrl.trim() ? form.appProxyUrl.trim() : null,
    model_list_prefix: form.modelListPrefix,
    kiro_preferred_endpoint: normalizeKiroPreferredEndpoint(form.kiroPreferredEndpoint),
    log_level: form.logLevel,
    retryable_failure_cooldown_secs: parseRetryableFailureCooldownSecs(
      form.retryableFailureCooldownSecs,
    ),
    upstream_no_data_timeout_secs: parseUpstreamNoDataTimeoutSecs(
      form.upstreamNoDataTimeoutSecs,
    ),
    tray_token_rate: form.trayTokenRate,
    upstream_strategy: toUpstreamStrategyPayload(form.upstreamStrategy),
    upstreams: form.upstreams.map((upstream) => {
      const providers = normalizeProviders(upstream.providers);
      const apiKeys = parseApiKeysInput(upstream.apiKeys);
      const omitNetworkFields = isAccountBackedProviderSet(providers);
      return {
        id: upstream.id.trim(),
        providers,
        base_url: omitNetworkFields ? "" : upstream.baseUrl.trim(),
        api_keys: apiKeys.length ? apiKeys : undefined,
        kiro_account_id: null,
        codex_account_id: null,
        filter_prompt_cache_retention: upstream.filterPromptCacheRetention,
        filter_safety_identifier: upstream.filterSafetyIdentifier,
        use_chat_completions_for_responses: upstream.useChatCompletionsForResponses,
        rewrite_developer_role_to_system: upstream.rewriteDeveloperRoleToSystem,
        preferred_endpoint: normalizeKiroPreferredEndpoint(upstream.preferredEndpoint),
        proxy_url: omitNetworkFields
          ? null
          : upstream.proxyUrl.trim()
            ? upstream.proxyUrl.trim()
            : null,
        priority: parseOptionalInt(upstream.priority),
        enabled: upstream.enabled,
        model_mappings: toModelMappingPayload(upstream.modelMappings),
        convert_from_map: normalizeConvertFromMap(upstream.convertFromMap, providers),
        overrides: toOverridesPayload(upstream.overrides),
      };
    }),
  };
}

function isSingleProvider(upstream: UpstreamForm, provider: "kiro" | "codex") {
  const providers = normalizeProviders(upstream.providers);
  return providers.length === 1 && providers[0] === provider;
}

export function syncAccountBackedUpstreams(
  upstreams: UpstreamForm[],
  accountState: { hasKiroAccount: boolean; hasCodexAccount: boolean },
) {
  const filtered = upstreams.filter((upstream) => {
    if (isSingleProvider(upstream, "kiro")) {
      return accountState.hasKiroAccount;
    }
    if (isSingleProvider(upstream, "codex")) {
      return accountState.hasCodexAccount;
    }
    return true;
  });

  const next = [...filtered];
  if (accountState.hasKiroAccount && !next.some((upstream) => isSingleProvider(upstream, "kiro"))) {
    next.push(createAccountBackedUpstream("kiro"));
  }
  if (accountState.hasCodexAccount && !next.some((upstream) => isSingleProvider(upstream, "codex"))) {
    next.push(createAccountBackedUpstream("codex"));
  }
  if (
    next.length === upstreams.length &&
    next.every((upstream, index) => upstream === upstreams[index])
  ) {
    return upstreams;
  }
  return next;
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
  if (!isValidRetryableFailureCooldownSecs(form.retryableFailureCooldownSecs)) {
    return {
      valid: false,
      message: m.error_retryable_failure_cooldown_secs_integer(),
    };
  }
  if (!isValidUpstreamNoDataTimeoutSecs(form.upstreamNoDataTimeoutSecs)) {
    return {
      valid: false,
      message: m.error_upstream_no_data_timeout_secs_integer(),
    };
  }
  const upstreamStrategyError = validateUpstreamStrategy(form.upstreamStrategy);
  if (upstreamStrategyError) {
    return { valid: false, message: upstreamStrategyError };
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
      provider === "kiro" || provider === "codex",
    );
    if (specialProviders.length && providers.length > 1) {
      return {
        valid: false,
        message: m.error_upstream_provider_required({ id }),
      };
    }
    if (specialProviders.length && parseApiKeysInput(upstream.apiKeys).length > 1) {
      return {
        valid: false,
        message: m.error_upstream_multiple_api_keys_unsupported({ id }),
      };
    }
    if (providers.some((provider) => !SUPPORTED_PROVIDERS.has(provider))) {
      return { valid: false, message: m.error_upstream_provider_required({ id }) };
    }

    const canOmitBaseUrl =
      providers.length === 1 &&
      (providers[0] === "kiro" || providers[0] === "codex");
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

function toUpstreamStrategyForm(strategy: UpstreamStrategy): ConfigForm["upstreamStrategy"] {
  switch (strategy.dispatch.type) {
    case "serial":
      return {
        order: strategy.order,
        dispatchType: "serial",
        hedgeDelayMs: String(DEFAULT_HEDGE_DELAY_MS),
        maxParallel: String(DEFAULT_MAX_PARALLEL),
      };
    case "hedged":
      return {
        order: strategy.order,
        dispatchType: "hedged",
        hedgeDelayMs: String(strategy.dispatch.delay_ms),
        maxParallel: String(strategy.dispatch.max_parallel),
      };
    case "race":
      return {
        order: strategy.order,
        dispatchType: "race",
        hedgeDelayMs: String(DEFAULT_HEDGE_DELAY_MS),
        maxParallel: String(strategy.dispatch.max_parallel),
      };
  }
}

function toUpstreamStrategyPayload(
  strategy: ConfigForm["upstreamStrategy"],
): UpstreamStrategy {
  return {
    order: strategy.order,
    dispatch: toUpstreamDispatchPayload(strategy),
  };
}

function toUpstreamDispatchPayload(
  strategy: ConfigForm["upstreamStrategy"],
): UpstreamDispatchStrategy {
  switch (strategy.dispatchType) {
    case "serial":
      return { type: "serial" };
    case "hedged":
      return {
        type: "hedged",
        delay_ms: parsePositiveInteger(strategy.hedgeDelayMs, DEFAULT_HEDGE_DELAY_MS),
        max_parallel: parseMinParallel(strategy.maxParallel, DEFAULT_MAX_PARALLEL),
      };
    case "race":
      return {
        type: "race",
        max_parallel: parseMinParallel(strategy.maxParallel, DEFAULT_MAX_PARALLEL),
      };
  }
}

function validateUpstreamStrategy(strategy: ConfigForm["upstreamStrategy"]) {
  if (strategy.dispatchType === "serial") {
    return "";
  }
  if (strategy.dispatchType === "hedged" && !isPositiveInteger(strategy.hedgeDelayMs)) {
    return m.error_upstream_strategy_delay_ms_positive_integer();
  }
  if (!isValidMinParallel(strategy.maxParallel)) {
    return m.error_upstream_strategy_max_parallel_min({
      min: String(MIN_PARALLEL_ATTEMPTS),
    });
  }
  return "";
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

function isValidRetryableFailureCooldownSecs(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return false;
  }
  return NON_NEGATIVE_INTEGER_PATTERN.test(trimmed);
}

function parseRetryableFailureCooldownSecs(value: string) {
  const trimmed = value.trim();
  if (!NON_NEGATIVE_INTEGER_PATTERN.test(trimmed)) {
    return 15;
  }
  const number = Number.parseInt(trimmed, 10);
  return Number.isFinite(number) ? number : 15;
}

function isValidUpstreamNoDataTimeoutSecs(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return false;
  }
  if (!NON_NEGATIVE_INTEGER_PATTERN.test(trimmed)) {
    return false;
  }
  const number = Number.parseInt(trimmed, 10);
  return Number.isFinite(number) && number >= MIN_UPSTREAM_NO_DATA_TIMEOUT_SECS;
}

function parseUpstreamNoDataTimeoutSecs(value: string) {
  const trimmed = value.trim();
  if (!NON_NEGATIVE_INTEGER_PATTERN.test(trimmed)) {
    return DEFAULT_UPSTREAM_NO_DATA_TIMEOUT_SECS;
  }
  const number = Number.parseInt(trimmed, 10);
  return Number.isFinite(number) ? number : DEFAULT_UPSTREAM_NO_DATA_TIMEOUT_SECS;
}

function isPositiveInteger(value: string) {
  return POSITIVE_INTEGER_PATTERN.test(value.trim());
}

function parsePositiveInteger(value: string, fallback: number) {
  const trimmed = value.trim();
  if (!POSITIVE_INTEGER_PATTERN.test(trimmed)) {
    return fallback;
  }
  const number = Number.parseInt(trimmed, 10);
  return Number.isFinite(number) ? number : fallback;
}

function isValidMinParallel(value: string) {
  const parsed = parsePositiveInteger(value, 0);
  return parsed >= MIN_PARALLEL_ATTEMPTS;
}

function parseMinParallel(value: string, fallback: number) {
  const parsed = parsePositiveInteger(value, fallback);
  return parsed >= MIN_PARALLEL_ATTEMPTS ? parsed : fallback;
}
