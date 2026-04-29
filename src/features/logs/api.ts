import { invoke } from "@tauri-apps/api/core";

import type {
  RequestDetailCaptureState,
  RequestLogDetail,
} from "@/features/logs/types";

export async function readRequestDetailCapture() {
  return await invoke<RequestDetailCaptureState>("read_request_detail_capture");
}

export async function setRequestDetailCapture(enabled: boolean, permanent: boolean = false) {
  return await invoke<RequestDetailCaptureState>("set_request_detail_capture", { enabled, permanent });
}

export async function readRequestLogDetail(id: number) {
  return await invoke<RequestLogDetail>("read_request_log_detail", { id });
}
