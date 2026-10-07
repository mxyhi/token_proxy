import { invoke } from "@tauri-apps/api/core";

import type {
  RequestDetailCaptureState,
  RequestLogDetail,
  RequestLogBodyPage,
} from "@/features/logs/types";

export async function readRequestDetailCapture() {
  return await invoke<RequestDetailCaptureState>("read_request_detail_capture");
}

export async function setRequestDetailCapture(enabled: boolean) {
  return await invoke<RequestDetailCaptureState>("set_request_detail_capture", { enabled });
}

export async function readRequestLogDetail(id: number) {
  return await invoke<RequestLogDetail>("read_request_log_detail", { id });
}

export async function readRequestLogBodyPage(id: number, offset: number) {
  return await invoke<RequestLogBodyPage>("read_request_log_body_page", { id, offset });
}
