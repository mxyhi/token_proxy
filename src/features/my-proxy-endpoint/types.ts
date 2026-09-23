/**
 * my-proxy-endpoint —— 仪表盘「代理接入地址」栏的类型。
 *
 * 与 Rust 侧 src-tauri/src/my_proxy_endpoint.rs::MyProxyEndpointSnapshot（camelCase）对齐，
 * 刻意不依赖上游 features/config 的类型，避免上游形状变化波及本功能。
 */

export const MY_PROXY_ENDPOINT_FORMATS = ["openai", "anthropic", "gemini"] as const;

export type MyProxyEndpointFormat = (typeof MY_PROXY_ENDPOINT_FORMATS)[number];

export type MyProxyEndpointSnapshot = {
  /** 规范化后的监听主机（0.0.0.0 / 127.0.0.1 / localhost …）。 */
  listenHost: string;
  port: number;
  /** host 为 0.0.0.0 / :: 时为 true，表示可选局域网地址。 */
  scanEnabled: boolean;
  /** 生效 IP（由后端按回落规则解析）。 */
  effectiveIp: string;
  effectiveFormat: MyProxyEndpointFormat;
  /** 启动扫描得到的可选 IPv4 快照。 */
  availableIps: string[];
  /** 本地 IPC 专用；界面只渲染掩码，明文仅用于复制。 */
  apiKey: string | null;
  apiKeyConfigured: boolean;
};

export type MyProxyEndpointStatus = "loading" | "ready" | "degraded";

export type MyProxyEndpointCopyTarget = "url" | "ip" | "key";
