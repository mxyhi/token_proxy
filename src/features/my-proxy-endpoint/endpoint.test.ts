import { describe, expect, it } from "vitest";

import {
  composeUrl,
  formatSuffix,
  ipKind,
  isMyProxyEndpointFormat,
  isMyProxyEndpointSnapshot,
  isScanHost,
  maskKey,
  normalizeDisplayHost,
  pickPreferred,
  resolveSelectedIp,
} from "./endpoint";
import type { MyProxyEndpointSnapshot } from "./types";

function snapshot(overrides: Partial<MyProxyEndpointSnapshot> = {}): MyProxyEndpointSnapshot {
  return {
    listenHost: "0.0.0.0",
    port: 9208,
    scanEnabled: true,
    effectiveIp: "192.168.1.23",
    effectiveFormat: "openai",
    availableIps: ["192.168.1.23", "10.0.0.5", "127.0.0.1"],
    apiKey: null,
    apiKeyConfigured: false,
    ...overrides,
  };
}

describe("my-proxy-endpoint/endpoint", () => {
  it("maps each format to its base-url suffix", () => {
    expect(formatSuffix("openai")).toBe("/v1");
    expect(formatSuffix("anthropic")).toBe("");
    expect(formatSuffix("gemini")).toBe("/v1beta");
  });

  it("composes the full url per format", () => {
    expect(composeUrl("192.168.1.23", 9208, "openai")).toBe("http://192.168.1.23:9208/v1");
    expect(composeUrl("192.168.1.23", 9208, "anthropic")).toBe("http://192.168.1.23:9208");
    expect(composeUrl("192.168.1.23", 9208, "gemini")).toBe("http://192.168.1.23:9208/v1beta");
    expect(composeUrl("127.0.0.1", 9308, "openai")).toBe("http://127.0.0.1:9308/v1");
  });

  it("detects the scan gate hosts", () => {
    expect(isScanHost("0.0.0.0")).toBe(true);
    expect(isScanHost(" :: ")).toBe(true);
    expect(isScanHost("127.0.0.1")).toBe(false);
    expect(isScanHost("localhost")).toBe(false);
    expect(isScanHost("")).toBe(false);
  });

  it("normalizes the fixed display host", () => {
    expect(normalizeDisplayHost("")).toBe("127.0.0.1");
    expect(normalizeDisplayHost("  ")).toBe("127.0.0.1");
    expect(normalizeDisplayHost(" localhost ")).toBe("localhost");
  });

  it("classifies addresses", () => {
    expect(ipKind("127.0.0.1")).toBe("loopback");
    expect(ipKind("localhost")).toBe("loopback");
    expect(ipKind("192.168.1.23")).toBe("lan");
    expect(ipKind("10.0.0.5")).toBe("lan");
    expect(ipKind("100.64.0.7")).toBe("virtual");
    expect(ipKind("100.127.255.1")).toBe("virtual");
    expect(ipKind("172.20.0.3")).toBe("virtual");
    expect(ipKind("172.15.0.3")).toBe("lan");
    expect(ipKind("not-an-ip")).toBe("lan");
  });

  it("masks the key only when configured", () => {
    expect(maskKey(true, 4)).toBe("••••");
    expect(maskKey(false)).toBe("");
  });

  it("picks the preferred fallback ip", () => {
    expect(pickPreferred(["10.0.0.5", "172.20.0.3", "192.168.1.23"])).toBe("192.168.1.23");
    expect(pickPreferred(["10.0.0.5", "172.20.0.3"])).toBe("172.20.0.3");
    expect(pickPreferred(["10.0.0.5", "127.0.0.1"])).toBeNull();
    expect(pickPreferred([])).toBeNull();
  });

  it("resolves the select value defensively", () => {
    expect(resolveSelectedIp(snapshot())).toBe("192.168.1.23");
    expect(resolveSelectedIp(snapshot({ effectiveIp: "10.0.0.9" }))).toBe("192.168.1.23");
    expect(
      resolveSelectedIp(
        snapshot({ scanEnabled: false, listenHost: "127.0.0.1", effectiveIp: "127.0.0.1" })
      )
    ).toBe("127.0.0.1");
    expect(
      resolveSelectedIp(
        snapshot({ scanEnabled: false, listenHost: "localhost", effectiveIp: "localhost" })
      )
    ).toBe("localhost");
  });

  it("rejects malformed snapshots at runtime", () => {
    expect(isMyProxyEndpointSnapshot(undefined)).toBe(false);
    expect(isMyProxyEndpointSnapshot(null)).toBe(false);
    expect(isMyProxyEndpointSnapshot("oops")).toBe(false);
    expect(isMyProxyEndpointSnapshot({})).toBe(false);
    expect(isMyProxyEndpointSnapshot({ ...snapshot(), effectiveFormat: "nope" })).toBe(false);
    expect(isMyProxyEndpointSnapshot({ ...snapshot(), availableIps: [1, 2] })).toBe(false);
    expect(isMyProxyEndpointSnapshot(snapshot())).toBe(true);
  });

  it("narrows format strings", () => {
    expect(isMyProxyEndpointFormat("openai")).toBe(true);
    expect(isMyProxyEndpointFormat("gemini")).toBe(true);
    expect(isMyProxyEndpointFormat("gemini-pro")).toBe(false);
  });
});
