import { Badge } from "@/components/ui/badge";
import { m } from "@/paraglide/messages.js";

import type { ClientSetupInfo } from "./client-setup-state";
import { CodeBlock, DetailSection, PathList } from "./client-setup-ui";

function OpenCodeModels({ models }: { models: readonly string[] }) {
  if (models.length === 0) {
    return (
      <div
        data-slot="client-setup-opencode-models"
        className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground"
      >
        {m.client_setup_opencode_models_empty()}
      </div>
    );
  }

  return (
    <div data-slot="client-setup-opencode-models" className="flex flex-wrap gap-1.5">
      {models.map((model) => (
        <Badge key={model} variant="outline" className="font-mono text-xs">
          {model}
        </Badge>
      ))}
    </div>
  );
}

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
            `auth.json: OPENAI_API_KEY = "${
              setup.codex_api_key_configured ? "••••••••" : m.client_setup_not_configured()
            }"`,
          ]}
        />
      </DetailSection>
    </div>
  );
}

export function OpenCodeSetupDetails({ setup }: { setup: ClientSetupInfo }) {
  return (
    <div data-slot="client-setup-opencode-details" className="space-y-4">
      <DetailSection label={m.client_setup_target_file_label()}>
        <PathList paths={[setup.opencode_config_path, setup.opencode_auth_path]} />
      </DetailSection>
      <DetailSection label={m.client_setup_effective_config_label()}>
        <CodeBlock
          lines={[
            `provider.${setup.opencode_provider_id}.npm = "@ai-sdk/openai-compatible"`,
            `provider.${setup.opencode_provider_id}.options.baseURL = "${
              setup.opencode_provider_base_url
            }"`,
            `auth.json: ${setup.opencode_provider_id}.key = "${
              setup.opencode_api_key_configured ? "••••••••" : m.client_setup_not_configured()
            }"`,
          ]}
        />
      </DetailSection>
      <DetailSection label={m.client_setup_opencode_models_label()}>
        <OpenCodeModels models={setup.opencode_models} />
      </DetailSection>
    </div>
  );
}
