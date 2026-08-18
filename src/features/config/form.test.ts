import { describe, expect, it } from "vitest";

import {
  EMPTY_FORM,
  createEmptyUpstream,
  extractConfigExtras,
  mergeConfigExtras,
  toForm,
  toPayload,
  validate,
} from "@/features/config/form";

describe("config/form", () => {
  it("validates required host", () => {
    expect(validate({ ...EMPTY_FORM, host: "   " }).valid).toBe(false);
  });

  it("validates port range", () => {
    expect(validate({ ...EMPTY_FORM, port: "70000" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, port: "0" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, port: "9208" }).valid).toBe(true);
  });

  it("validates request body limit as integer 1..1024 MiB", () => {
    expect(validate({ ...EMPTY_FORM, maxRequestBodyMib: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, maxRequestBodyMib: "0" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, maxRequestBodyMib: "1025" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, maxRequestBodyMib: "1" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, maxRequestBodyMib: "100" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, maxRequestBodyMib: "1024" }).valid).toBe(true);
  });

  it("validates retryable failure cooldown as non-negative integer", () => {
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "-1" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "0" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "15" }).valid).toBe(true);
  });

  it("validates same-upstream retry count as integer 0..5", () => {
    expect(validate({ ...EMPTY_FORM, sameUpstreamRetryCount: "-1" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, sameUpstreamRetryCount: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, sameUpstreamRetryCount: "6" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, sameUpstreamRetryCount: "0" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, sameUpstreamRetryCount: "1" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, sameUpstreamRetryCount: "5" }).valid).toBe(true);
  });

  it("validates stream first output timeout as integer >= 1", () => {
    expect(validate({ ...EMPTY_FORM, streamFirstOutputTimeoutSecs: "-1" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, streamFirstOutputTimeoutSecs: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, streamFirstOutputTimeoutSecs: "0" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, streamFirstOutputTimeoutSecs: "1" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, streamFirstOutputTimeoutSecs: "60" }).valid).toBe(true);
  });

  it("validates synchronous response timeout as integer >= 1", () => {
    expect(validate({ ...EMPTY_FORM, syncResponseTimeoutSecs: "-1" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, syncResponseTimeoutSecs: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, syncResponseTimeoutSecs: "0" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, syncResponseTimeoutSecs: "1" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, syncResponseTimeoutSecs: "300" }).valid).toBe(true);
  });

  it("requires upstream id for enabled upstreams", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "";
    upstream.enabled = true;
    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });

    expect(result.valid).toBe(false);
    expect(result.message).not.toBe("");
  });

  it("rejects unsupported provider in enabled upstreams", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "legacy-1";
    upstream.enabled = true;
    upstream.providers = ["legacy-provider"];

    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });

    expect(result.valid).toBe(false);
  });

  it("allows enabled account-backed upstreams without base urls when account is bound", () => {
    const kiroUpstream = createEmptyUpstream();
    kiroUpstream.id = "kiro-1";
    kiroUpstream.enabled = true;
    kiroUpstream.providers = ["kiro"];
    kiroUpstream.accountId = "kiro-acc";

    const codexUpstream = createEmptyUpstream();
    codexUpstream.id = "codex-1";
    codexUpstream.enabled = true;
    codexUpstream.providers = ["codex"];
    codexUpstream.accountId = "codex-acc";

    const antigravityUpstream = createEmptyUpstream();
    antigravityUpstream.id = "antigravity-1";
    antigravityUpstream.enabled = true;
    antigravityUpstream.providers = ["antigravity"];
    antigravityUpstream.baseUrl = "";

    expect(validate({ ...EMPTY_FORM, upstreams: [kiroUpstream] }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, upstreams: [codexUpstream] }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, upstreams: [antigravityUpstream] }).valid).toBe(true);
  });

  it("treats disabled upstream as draft (still requires id)", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "u1";
    upstream.enabled = false;
    upstream.providers = [];
    upstream.baseUrl = "";

    expect(validate({ ...EMPTY_FORM, upstreams: [upstream] }).valid).toBe(true);
  });

  it("creates new upstream with default airouter template", () => {
    const upstream = createEmptyUpstream();

    expect(upstream.id).toBe("airouter.mxyhi.com");
    expect(upstream.baseUrl).toBe("https://airouter.mxyhi.com");
    expect(upstream.providers).toEqual(["openai", "openai-response", "anthropic", "gemini"]);
    expect(upstream.priority).toBe("19");
    expect(upstream.enabled).toBe(false);
    expect(validate({ ...EMPTY_FORM, upstreams: [upstream] }).valid).toBe(true);
  });

  it("extracts and merges unknown config keys as extras", () => {
    const payload = toPayload(EMPTY_FORM);
    const configWithExtras = {
      ...payload,
      foo: 1,
      bar: { nested: true },
      upstream_no_data_timeout_secs: 120,
      openai_response_header_timeout_secs: 0,
    };

    const extras = extractConfigExtras(configWithExtras);
    expect(extras).toEqual({ foo: 1, bar: { nested: true } });

    const merged = mergeConfigExtras(payload, extras);
    expect(merged).toMatchObject({
      foo: 1,
      bar: { nested: true },
      host: payload.host,
      port: payload.port,
    });
  });

  it("normalizes payload (trim + de-dup providers + sanitize convert_from_map)", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "  upstream-1 ";
    upstream.providers = [" openai ", "openai", "", " openai-response "];
    upstream.baseUrl = " https://example.com ";
    upstream.apiKeys = "   ";
    upstream.convertFromMap = {
      openai: ["openai_chat"],
      unknown: ["gemini"],
    };

    const payload = toPayload({
      ...EMPTY_FORM,
      host: " 127.0.0.1 ",
      localApiKey: " ",
      corsEnabled: true,
      modelListPrefix: true,
      upstreams: [upstream],
    });

    expect(payload.host).toBe("127.0.0.1");
    expect(payload.local_api_key).toBeNull();
    expect(payload.cors_enabled).toBe(true);
    expect(payload.model_list_prefix).toBe(true);
    expect(payload.retryable_failure_cooldown_secs).toBe(15);
    expect(payload.same_upstream_retry_count).toBe(1);
    expect(payload.codex_session_scoped_cooldown_enabled).toBe(false);
    expect(payload.xai_inject_x_search).toBe(false);
    expect(payload.stream_first_output_timeout_secs).toBe(60);
    expect(payload.sync_response_timeout_secs).toBe(300);
    expect("upstream_no_data_timeout_secs" in payload).toBe(false);
    expect("openai_response_header_timeout_secs" in payload).toBe(false);
    expect("model_discovery_refresh_secs" in payload).toBe(false);
    expect(payload.upstreams[0]?.id).toBe("upstream-1");
    expect(payload.upstreams[0]?.providers).toEqual(["openai", "openai-response"]);
    expect(payload.upstreams[0]?.base_url).toBe("https://example.com");
    expect(payload.upstreams[0]?.credential).toEqual({ type: "passthrough" });
    // openai_chat 对 openai 是 native 格式，应被清理；unknown provider 也应被丢弃。
    expect(payload.upstreams[0]?.convert_from_map).toBeUndefined();
  });

  it("loads and serializes the xAI X Search injection switch", () => {
    const source = { ...toPayload(EMPTY_FORM), xai_inject_x_search: true };

    const form = toForm(source);
    expect(form.xaiInjectXSearch).toBe(true);

    const payload = toPayload(form);
    expect(payload.xai_inject_x_search).toBe(true);
  });

  it("serializes multiple upstream api keys into credential", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "multi-key";
    upstream.apiKeys = " key-a, key-b, key-a ";

    const payload = toPayload({
      ...EMPTY_FORM,
      upstreams: [upstream],
    });

    expect(payload.upstreams[0]?.credential).toEqual({
      type: "api_keys",
      api_keys: ["key-a", "key-b"],
    });
  });

  it("loads and serializes upstream available model restrictions", () => {
    const source = toPayload(EMPTY_FORM);
    source.upstreams = [
      {
        id: "openai-main",
        providers: ["openai"],
        base_url: "https://example.com",
        credential: { type: "passthrough" },
        proxy_url: null,
        priority: 0,
        enabled: true,
        available_models: [" gpt-5.4-mini ", "gpt-5.4", "gpt-5.4"],
        model_mappings: {},
      },
    ];

    const form = toForm(source);
    expect(form.upstreams[0]?.availableModelsMode).toBe("selected");
    expect(form.upstreams[0]?.availableModels).toEqual(["gpt-5.4", "gpt-5.4-mini"]);

    const payload = toPayload(form);
    expect(payload.upstreams[0]?.available_models).toEqual(["gpt-5.4", "gpt-5.4-mini"]);
  });

  it("omits available models when all models are allowed", () => {
    const upstream = createEmptyUpstream();
    upstream.availableModelsMode = "all";
    upstream.availableModels = ["gpt-5.4"];

    const payload = toPayload({ ...EMPTY_FORM, upstreams: [upstream] });

    expect(payload.upstreams[0]?.available_models).toBeUndefined();
  });

  it("requires a model in selected-model mode", () => {
    const upstream = createEmptyUpstream();
    upstream.enabled = true;
    upstream.availableModelsMode = "selected";
    upstream.availableModels = [];

    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });

    expect(result.valid).toBe(false);
    expect(result.message).toContain(upstream.id);
  });

  it("serializes request body limit as bytes", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      maxRequestBodyMib: "200",
    });

    expect(payload.max_request_body_bytes).toBe(200 * 1024 * 1024);
  });

  it("defaults request body limit to 100 MiB when config omits it", () => {
    expect(EMPTY_FORM.maxRequestBodyMib).toBe("100");
    const form = toForm({
      host: "127.0.0.1",
      port: 9208,
      local_api_key: null,
      app_proxy_url: null,
      upstreams: [],
      tray_token_rate: { enabled: true, format: "split" },
      upstream_strategy: { order: "fill_first", dispatch: { type: "serial" } },
    });
    expect(form.maxRequestBodyMib).toBe("100");
  });

  it("serializes retryable failure cooldown seconds", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      retryableFailureCooldownSecs: "30",
    });

    expect(payload.retryable_failure_cooldown_secs).toBe(30);
  });

  it("serializes same-upstream retry count", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      sameUpstreamRetryCount: "3",
    });

    expect(payload.same_upstream_retry_count).toBe(3);
  });

  it("serializes codex session-scoped cooldown switch", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      codexSessionScopedCooldownEnabled: true,
    });

    expect(payload.codex_session_scoped_cooldown_enabled).toBe(true);
  });

  it("defaults split timeout seconds when config omits them", () => {
    expect(EMPTY_FORM.streamFirstOutputTimeoutSecs).toBe("60");
    expect(EMPTY_FORM.syncResponseTimeoutSecs).toBe("300");

    const form = toForm({
      host: "127.0.0.1",
      port: 9208,
      local_api_key: null,
      app_proxy_url: null,
      upstreams: [
        {
          id: "multi-key",
          providers: ["openai"],
          base_url: "https://example.com",
          credential: { type: "api_keys", api_keys: ["key-a", "key-b"] },
          proxy_url: null,
          priority: null,
          enabled: true,
          model_mappings: {},
        },
      ],
      tray_token_rate: {
        enabled: true,
        format: "split",
      },
      upstream_strategy: {
        order: "fill_first",
        dispatch: {
          type: "serial",
        },
      },
    });

    expect(form.streamFirstOutputTimeoutSecs).toBe("60");
    expect(form.syncResponseTimeoutSecs).toBe("300");
    expect(form.corsEnabled).toBe(false);
    expect(form.modelListPrefix).toBe(true);
    expect(form.codexSessionScopedCooldownEnabled).toBe(false);
    expect(form.upstreams[0]?.apiKeys).toBe("key-a, key-b");
    expect(form.upstreamStrategy).toEqual({
      order: "fill_first",
      dispatchType: "serial",
      hedgeDelayMs: "2000",
      maxParallel: "2",
    });
  });

  it("round-trips account credential bindings for kiro/codex/xai", () => {
    const form = toForm({
      host: "127.0.0.1",
      port: 9208,
      local_api_key: null,
      app_proxy_url: null,
      upstreams: [
        {
          id: "kiro-1",
          providers: ["kiro"],
          base_url: "",
          credential: { type: "account", provider: "kiro", account_id: "kiro-primary.json" },
          proxy_url: "http://127.0.0.1:7890",
          priority: 10,
          enabled: true,
          model_mappings: {},
        },
        {
          id: "codex-1",
          providers: ["codex"],
          base_url: "",
          credential: { type: "account", provider: "codex", account_id: "codex-primary.json" },
          proxy_url: null,
          priority: 20,
          enabled: true,
          model_mappings: {},
        },
        {
          id: "xai-1",
          providers: ["xai"],
          base_url: "",
          credential: {
            type: "account",
            provider: "xai",
            account_id: "xai-user@example.com",
          },
          proxy_url: null,
          priority: 30,
          enabled: true,
          model_mappings: {},
        },
      ],
      tray_token_rate: {
        enabled: true,
        format: "split",
      },
      upstream_strategy: {
        order: "fill_first",
        dispatch: { type: "serial" },
      },
    });

    expect(form.upstreams[0]?.accountId).toBe("kiro-primary.json");
    expect(form.upstreams[0]?.proxyUrl).toBe("http://127.0.0.1:7890");
    expect(form.upstreams[1]?.accountId).toBe("codex-primary.json");
    expect(form.upstreams[2]?.accountId).toBe("xai-user@example.com");

    const payload = toPayload(form);
    expect(payload.upstreams[0]?.credential).toEqual({
      type: "account",
      provider: "kiro",
      account_id: "kiro-primary.json",
    });
    expect(payload.upstreams[0]?.base_url).toBe("");
    expect(payload.upstreams[0]?.proxy_url).toBe("http://127.0.0.1:7890");
    expect(payload.upstreams[1]?.credential).toEqual({
      type: "account",
      provider: "codex",
      account_id: "codex-primary.json",
    });
    expect(payload.upstreams[2]?.credential).toEqual({
      type: "account",
      provider: "xai",
      account_id: "xai-user@example.com",
    });
  });

  it("rejects enabled account upstream without account_id", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "kiro-empty";
    upstream.providers = ["kiro"];
    upstream.accountId = "";
    upstream.enabled = true;

    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });
    expect(result.valid).toBe(false);
    expect(result.message).toContain("kiro-empty");
  });

  it("rejects disabled account upstream without account_id (no draft bypass)", () => {
    // 账户型 credential 不可靠「禁用草稿」绕过 account_id 必填。
    const upstream = createEmptyUpstream();
    upstream.id = "codex-disabled-empty";
    upstream.providers = ["codex"];
    upstream.accountId = "";
    upstream.enabled = false;

    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });
    expect(result.valid).toBe(false);
    expect(result.message).toContain("codex-disabled-empty");
  });

  it("still allows disabled ordinary upstream drafts without base_url", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "openai-draft";
    upstream.providers = ["openai"];
    upstream.baseUrl = "";
    upstream.enabled = false;

    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });
    expect(result.valid).toBe(true);
  });

  it("omits base_url for account-backed providers but keeps proxy", () => {
    const kiroUpstream = createEmptyUpstream();
    kiroUpstream.id = "kiro-1";
    kiroUpstream.providers = ["kiro"];
    kiroUpstream.accountId = "acc-1";
    kiroUpstream.baseUrl = "https://should-not-survive.example.com";
    kiroUpstream.proxyUrl = "http://127.0.0.1:7890";

    const payload = toPayload({
      ...EMPTY_FORM,
      upstreams: [kiroUpstream],
    });

    expect(payload.upstreams[0]?.base_url).toBe("");
    expect(payload.upstreams[0]?.proxy_url).toBe("http://127.0.0.1:7890");
    expect(payload.upstreams[0]?.credential).toEqual({
      type: "account",
      provider: "kiro",
      account_id: "acc-1",
    });
  });

  it("serializes split timeout seconds", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      streamFirstOutputTimeoutSecs: "45",
      syncResponseTimeoutSecs: "180",
    });

    expect(payload.stream_first_output_timeout_secs).toBe(45);
    expect(payload.sync_response_timeout_secs).toBe(180);
  });

  it("serializes structured upstream strategy", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      upstreamStrategy: {
        order: "round_robin",
        dispatchType: "hedged",
        hedgeDelayMs: "1500",
        maxParallel: "3",
      },
    });

    expect(payload.upstream_strategy).toEqual({
      order: "round_robin",
      dispatch: {
        type: "hedged",
        delay_ms: 1500,
        max_parallel: 3,
      },
    });
  });

  it("validates hedged delay as positive integer", () => {
    expect(
      validate({
        ...EMPTY_FORM,
        upstreamStrategy: {
          ...EMPTY_FORM.upstreamStrategy,
          dispatchType: "hedged",
          hedgeDelayMs: "0",
        },
      }).valid
    ).toBe(false);

    expect(
      validate({
        ...EMPTY_FORM,
        upstreamStrategy: {
          ...EMPTY_FORM.upstreamStrategy,
          dispatchType: "hedged",
          hedgeDelayMs: "1",
        },
      }).valid
    ).toBe(true);
  });

  it("validates race and hedged max parallel as integer >= 2", () => {
    expect(
      validate({
        ...EMPTY_FORM,
        upstreamStrategy: {
          ...EMPTY_FORM.upstreamStrategy,
          dispatchType: "hedged",
          maxParallel: "1",
        },
      }).valid
    ).toBe(false);

    expect(
      validate({
        ...EMPTY_FORM,
        upstreamStrategy: {
          ...EMPTY_FORM.upstreamStrategy,
          dispatchType: "race",
          maxParallel: "1",
        },
      }).valid
    ).toBe(false);

    expect(
      validate({
        ...EMPTY_FORM,
        upstreamStrategy: {
          ...EMPTY_FORM.upstreamStrategy,
          dispatchType: "race",
          maxParallel: "2",
        },
      }).valid
    ).toBe(true);
  });
  it("serializes openai compatibility upstream flags", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "glm-coding-plan";
    upstream.providers = ["openai-response"];
    upstream.baseUrl = "https://open.bigmodel.cn/api/coding/paas/v4";
    upstream.enabled = true;
    upstream.useChatCompletionsForResponses = true;
    upstream.rewriteDeveloperRoleToSystem = true;

    const payload = toPayload({
      ...EMPTY_FORM,
      upstreams: [upstream],
    });

    expect(payload.upstreams[0]?.use_chat_completions_for_responses).toBe(true);
    expect(payload.upstreams[0]?.rewrite_developer_role_to_system).toBe(true);
  });
});
