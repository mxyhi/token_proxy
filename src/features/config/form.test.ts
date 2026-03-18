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

  it("validates retryable failure cooldown as non-negative integer", () => {
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "-1" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "0" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, retryableFailureCooldownSecs: "15" }).valid).toBe(true);
  });

  it("validates upstream no data timeout as integer >= 3", () => {
    expect(validate({ ...EMPTY_FORM, upstreamNoDataTimeoutSecs: "-1" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, upstreamNoDataTimeoutSecs: "" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, upstreamNoDataTimeoutSecs: "0" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, upstreamNoDataTimeoutSecs: "2" }).valid).toBe(false);
    expect(validate({ ...EMPTY_FORM, upstreamNoDataTimeoutSecs: "3" }).valid).toBe(true);
    expect(validate({ ...EMPTY_FORM, upstreamNoDataTimeoutSecs: "120" }).valid).toBe(true);
  });

  it("requires upstream id for enabled upstreams", () => {
    const upstream = createEmptyUpstream();
    const result = validate({ ...EMPTY_FORM, upstreams: [upstream] });

    expect(result.valid).toBe(false);
    expect(result.message).not.toBe("");
  });

  it("treats disabled upstream as draft (still requires id)", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "u1";
    upstream.enabled = false;
    upstream.providers = [];
    upstream.baseUrl = "";

    expect(validate({ ...EMPTY_FORM, upstreams: [upstream] }).valid).toBe(true);
  });

  it("creates new upstream as disabled draft", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "openai-1";

    expect(upstream.enabled).toBe(false);
    expect(validate({ ...EMPTY_FORM, upstreams: [upstream] }).valid).toBe(true);
  });

  it("extracts and merges unknown config keys as extras", () => {
    const payload = toPayload(EMPTY_FORM);
    const configWithExtras = { ...payload, foo: 1, bar: { nested: true } };

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
      upstreams: [upstream],
    });

    expect(payload.host).toBe("127.0.0.1");
    expect(payload.local_api_key).toBeNull();
    expect(payload.retryable_failure_cooldown_secs).toBe(15);
    expect(payload.upstream_no_data_timeout_secs).toBe(120);
    expect(payload.upstreams[0]?.id).toBe("upstream-1");
    expect(payload.upstreams[0]?.providers).toEqual(["openai", "openai-response"]);
    expect(payload.upstreams[0]?.base_url).toBe("https://example.com");
    expect(payload.upstreams[0]?.api_keys).toBeUndefined();
    // openai_chat 对 openai 是 native 格式，应被清理；unknown provider 也应被丢弃。
    expect(payload.upstreams[0]?.convert_from_map).toBeUndefined();
  });

  it("serializes multiple upstream api keys", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "multi-key";
    upstream.apiKeys = " key-a, key-b, key-a ";

    const payload = toPayload({
      ...EMPTY_FORM,
      upstreams: [upstream],
    });

    expect(payload.upstreams[0]?.api_keys).toEqual(["key-a", "key-b"]);
  });

  it("serializes retryable failure cooldown seconds", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      retryableFailureCooldownSecs: "30",
    });

    expect(payload.retryable_failure_cooldown_secs).toBe(30);
  });

  it("defaults upstream no data timeout seconds to 120 when config omits it", () => {
    expect(EMPTY_FORM.upstreamNoDataTimeoutSecs).toBe("120");

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
          api_keys: ["key-a", "key-b"],
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

    expect(form.upstreamNoDataTimeoutSecs).toBe("120");
    expect(form.upstreams[0]?.apiKeys).toBe("key-a, key-b");
    expect(form.upstreamStrategy).toEqual({
      order: "fill_first",
      dispatchType: "serial",
      hedgeDelayMs: "2000",
      maxParallel: "2",
    });
  });

  it("serializes upstream no data timeout seconds", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      upstreamNoDataTimeoutSecs: "45",
    });

    expect(payload.upstream_no_data_timeout_secs).toBe(45);
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
