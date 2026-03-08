import { describe, expect, it } from "vitest";

import {
  EMPTY_FORM,
  createEmptyUpstream,
  extractConfigExtras,
  mergeConfigExtras,
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
    upstream.apiKey = "   ";
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
    expect(payload.upstreams[0]?.id).toBe("upstream-1");
    expect(payload.upstreams[0]?.providers).toEqual(["openai", "openai-response"]);
    expect(payload.upstreams[0]?.base_url).toBe("https://example.com");
    expect(payload.upstreams[0]?.api_key).toBeNull();
    // openai_chat 对 openai 是 native 格式，应被清理；unknown provider 也应被丢弃。
    expect(payload.upstreams[0]?.convert_from_map).toBeUndefined();
  });

  it("serializes retryable failure cooldown seconds", () => {
    const payload = toPayload({
      ...EMPTY_FORM,
      retryableFailureCooldownSecs: "30",
    });

    expect(payload.retryable_failure_cooldown_secs).toBe(30);
  });
});
