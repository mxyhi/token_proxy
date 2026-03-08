import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

export type ClientSetupInfo = {
  proxy_http_base_url: string;
  claude_settings_path: string;
  claude_base_url: string;
  claude_auth_token_configured: boolean;
  codex_config_path: string;
  codex_auth_path: string;
  codex_disable_response_storage: boolean;
  codex_model: string;
  codex_model_provider: string;
  codex_model_reasoning_effort: string;
  codex_network_access: string;
  codex_preferred_auth_method: string;
  codex_provider_base_url: string;
  codex_provider_name: string;
  codex_provider_requires_openai_auth: boolean;
  codex_provider_wire_api: string;
  codex_api_key_configured: boolean;
  opencode_config_path: string;
  opencode_auth_path: string;
  opencode_provider_id: string;
  opencode_provider_base_url: string;
  opencode_models: string[];
  opencode_api_key_configured: boolean;
};

type ClientConfigWriteResult = {
  paths: string[];
};

export type RequestState = "idle" | "working" | "success" | "error";

export type ActionState = {
  state: RequestState;
  message: string;
  lastPath: string;
};

export type WriteCommand =
  | "write_claude_code_settings"
  | "write_codex_config"
  | "write_opencode_config";

export function toActionState(): ActionState {
  return { state: "idle", message: "", lastPath: "" };
}

export function useClientSetupPreview(savedAt: string) {
  const [previewState, setPreviewState] = useState<RequestState>("working");
  const [previewMessage, setPreviewMessage] = useState("");
  const [setup, setSetup] = useState<ClientSetupInfo | null>(null);
  const requestSeq = useRef(0);

  const fetchPreview = useCallback(async () => {
    const requestId = requestSeq.current + 1;
    requestSeq.current = requestId;
    try {
      const result = await invoke<ClientSetupInfo>("preview_client_setup");
      if (requestSeq.current !== requestId) {
        return;
      }
      setSetup(result);
      setPreviewState("success");
      setPreviewMessage("");
    } catch (error) {
      if (requestSeq.current !== requestId) {
        return;
      }
      setPreviewState("error");
      setPreviewMessage(parseError(error));
    }
  }, []);

  useEffect(() => {
    // 延后一拍启动异步预览请求，避免在 effect 同步执行路径中触发 hooks lint 的级联更新告警。
    const timerId = window.setTimeout(() => {
      void fetchPreview();
    }, 0);
    return () => window.clearTimeout(timerId);
  }, [fetchPreview, savedAt]);

  const loadPreview = useCallback(async () => {
    setPreviewState("working");
    setPreviewMessage("");
    await fetchPreview();
  }, [fetchPreview]);

  return { previewState, previewMessage, setup, loadPreview };
}

export function useWriteAction(command: WriteCommand, loadPreview: () => Promise<void>) {
  const [action, setAction] = useState<ActionState>(toActionState);

  const apply = useCallback(async () => {
    setAction({ state: "working", message: "", lastPath: "" });
    try {
      const result = await invoke<ClientConfigWriteResult>(command);
      const path = result.paths.join(", ");
      setAction({ state: "success", message: m.client_setup_apply_success({ path }), lastPath: path });
      await loadPreview();
    } catch (error) {
      setAction({ state: "error", message: parseError(error), lastPath: "" });
    }
  }, [command, loadPreview]);

  return { action, apply };
}
