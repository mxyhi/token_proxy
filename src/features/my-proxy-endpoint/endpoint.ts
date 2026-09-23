import {
  MY_PROXY_ENDPOINT_FORMATS,
  type MyProxyEndpointFormat,
  type MyProxyEndpointSnapshot,
} from "./types";

export const LOOPBACK_IP = "127.0.0.1";

/** 每个接口格式对应的 base URL 后缀（与运行时入站路径一致）。 */
const FORMAT_SUFFIXES: Record<MyProxyEndpointFormat, string> = {
  openai: "/v1",
  anthropic: "",
  gemini: "/v1beta",
};

export function isMyProxyEndpointFormat(value: string): value is MyProxyEndpointFormat {
  return (MY_PROXY_ENDPOINT_FORMATS as readonly string[]).includes(value);
}

export function formatSuffix(format: MyProxyEndpointFormat): string {
  return FORMAT_SUFFIXES[format];
}

/**
 * 运行时形状校验：IPC 返回异常（undefined / 缺字段 / 类型错误）时降级为不可用，
 * 避免在渲染期对 undefined 取值而崩溃（也保证组件在非 Tauri 环境安全）。
 */
export function isMyProxyEndpointSnapshot(value: unknown): value is MyProxyEndpointSnapshot {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<MyProxyEndpointSnapshot>;
  return (
    typeof candidate.listenHost === "string" &&
    typeof candidate.port === "number" &&
    typeof candidate.scanEnabled === "boolean" &&
    typeof candidate.effectiveIp === "string" &&
    isMyProxyEndpointFormat(candidate.effectiveFormat ?? "") &&
    Array.isArray(candidate.availableIps) &&
    candidate.availableIps.every((ip) => typeof ip === "string") &&
    typeof candidate.apiKeyConfigured === "boolean" &&
    (candidate.apiKey === null || typeof candidate.apiKey === "string")
  );
}

/** 组合完整接入 URL：http://<ip>:<port><后缀>。 */
export function composeUrl(ip: string, port: number, format: MyProxyEndpointFormat): string {
  return "http://" + ip + ":" + port + formatSuffix(format);
}

/** 后端扫描门控：仅 0.0.0.0 / :: 需要扫描局域网地址。 */
export function isScanHost(host: string): boolean {
  const trimmed = host.trim();
  return trimmed === "0.0.0.0" || trimmed === "::";
}

/** 非扫描态固定显示的监听主机；空串按回环处理。 */
export function normalizeDisplayHost(host: string): string {
  const trimmed = host.trim();
  return trimmed.length > 0 ? trimmed : LOOPBACK_IP;
}

export type IpKind = "loopback" | "lan" | "virtual";

function secondOctet(ip: string): number | null {
  const parts = ip.split(".");
  if (parts.length !== 4) {
    return null;
  }
  const parsed = Number.parseInt(parts[1] ?? "", 10);
  return Number.isFinite(parsed) ? parsed : null;
}

/**
 * 地址类型：回环 / 局域网 / 虚拟网卡。
 * 虚拟网卡覆盖 CGNAT-Tailscale 100.64.0.0/10 与常见的 172.16.0.0/12（WSL / Hyper-V / Docker NAT）。
 */
export function ipKind(ip: string): IpKind {
  if (ip === "localhost" || ip.startsWith("127.")) {
    return "loopback";
  }
  const octet = secondOctet(ip);
  if (octet !== null) {
    if (ip.startsWith("100.") && octet >= 64 && octet <= 127) {
      return "virtual";
    }
    if (ip.startsWith("172.") && octet >= 16 && octet <= 31) {
      return "virtual";
    }
  }
  return "lan";
}

/** Key 掩码：未配置返回空串，已配置返回固定长度圆点（明文永不进 DOM）。 */
export function maskKey(configured: boolean, length = 12): string {
  return configured ? "•".repeat(length) : "";
}

/** 回落后备：首个 192.* → 首个 172.* → null（与后端回落规则一致）。 */
export function pickPreferred(available: readonly string[]): string | null {
  const preferred = available.find((ip) => ip.startsWith("192."));
  if (preferred !== undefined) {
    return preferred;
  }
  return available.find((ip) => ip.startsWith("172.")) ?? null;
}

/**
 * 下拉选中值：优先使用后端解析的 effectiveIp；若它不在可选列表中（异常 / 旧配置），
 * 按回落规则补一个合法值，避免 Radix Select 出现空显示。
 */
export function resolveSelectedIp(snapshot: MyProxyEndpointSnapshot): string {
  if (!snapshot.scanEnabled) {
    return snapshot.effectiveIp;
  }
  if (snapshot.availableIps.includes(snapshot.effectiveIp)) {
    return snapshot.effectiveIp;
  }
  return pickPreferred(snapshot.availableIps) ?? normalizeDisplayHost(snapshot.listenHost);
}
