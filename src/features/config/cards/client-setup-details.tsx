import { m } from "@/paraglide/messages.js";

import type { ClientSetupInfo } from "./client-setup-state";
import { CodeBlock, DetailSection, PathList } from "./client-setup-ui";

export function ClaudeSetupDetails({ setup }: { setup: ClientSetupInfo }) {
  return (
    <div data-slot="client-setup-claude-details" className="space-y-4">
      <DetailSection label={m.client_setup_target_file_label()}>
        <PathList paths={[setup.claude_settings_path]} />
      </DetailSection>
      <DetailSection label={m.client_setup_effective_env_label()}>
        <CodeBlock
          lines={[
            `ANTHROPIC_BASE_URL=${setup.claude_base_url}`,
            `ANTHROPIC_MODEL=${setup.claude_model}`,
            `ANTHROPIC_AUTH_TOKEN=${
              setup.claude_auth_token_configured ? "••••••••" : m.client_setup_not_configured()
            }`,
          ]}
        />
      </DetailSection>
    </div>
  );
}

export function CodexSetupDetails({ setup }: { setup: ClientSetupInfo }) {
  const modelCatalogUrl =
    setup.codex_model_catalog_url ??
    `${setup.proxy_http_base_url.replace(/\/+$/, "")}/codex/models.json`;
  return (
    <div data-slot="client-setup-codex-details" className="space-y-4">
      <DetailSection label={m.client_setup_target_file_label()}>
        <PathList paths={[setup.codex_config_path, setup.codex_auth_path]} />
      </DetailSection>
      <DetailSection label={m.client_setup_effective_config_label()}>
        <CodeBlock
          lines={[
            `disable_response_storage = ${setup.codex_disable_response_storage ? "true" : "false"}`,
            `model = "${setup.codex_model}"`,
            `model_provider = "${setup.codex_model_provider}"`,
            `model_reasoning_effort = "${setup.codex_model_reasoning_effort}"`,
            `network_access = "${setup.codex_network_access}"`,
            `preferred_auth_method = "${setup.codex_preferred_auth_method}"`,
            `[model_providers.${setup.codex_model_provider}]`,
            `base_url = "${setup.codex_provider_base_url}"`,
            `name = "${setup.codex_provider_name}"`,
            `requires_openai_auth = ${setup.codex_provider_requires_openai_auth ? "true" : "false"}`,
            `wire_api = "${setup.codex_provider_wire_api}"`,
            `model_catalog_url = "${modelCatalogUrl}"`,
            // 与后端写入一致：有本地 key 时同步 auth_mode，避免旧 ChatGPT 登录态遮住 API Key。
            ...(setup.codex_api_key_configured ? ['auth.json: auth_mode = "apikey"'] : []),
            `auth.json: OPENAI_API_KEY = "${
              setup.codex_api_key_configured ? "••••••••" : m.client_setup_not_configured()
            }"`,
          ]}
        />
      </DetailSection>
    </div>
  );
}
