import { invoke } from "@tauri-apps/api/core";

import type { MyProxyEndpointSnapshot } from "./types";

/** 读取快照（首次调用会触发一次扫描并落盘）。 */
export async function readMyProxyEndpoint(): Promise<MyProxyEndpointSnapshot> {
  return invoke<MyProxyEndpointSnapshot>("my_proxy_endpoint_snapshot");
}

/** 落盘一次选择；只更新 my_proxy_endpoint 段，不触发代理 reload。 */
export async function selectMyProxyEndpoint(
  currentIp: string,
  currentFormat: string
): Promise<MyProxyEndpointSnapshot> {
  return invoke<MyProxyEndpointSnapshot>("my_proxy_endpoint_select", {
    currentIp,
    currentFormat,
  });
}
