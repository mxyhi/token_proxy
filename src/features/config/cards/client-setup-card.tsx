import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type ClientSetupInfo = {
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
};

type ClientConfigWriteResult = {
  paths: string[];
};

type RequestState = "idle" | "working" | "success" | "error";

type ActionState = {
  state: RequestState;
  message: string;
  lastPath: string;
};

function toActionState(): ActionState {
  return { state: "idle", message: "", lastPath: "" };
}

function toBadgeVariant(state: RequestState) {
  if (state === "success") return "default";
  if (state === "error") return "destructive";
  if (state === "working") return "secondary";
  return "outline";
}

function toBadgeLabel(state: RequestState) {
  if (state === "success") return m.client_setup_status_success();
  if (state === "error") return m.client_setup_status_error();
  if (state === "working") return m.client_setup_status_working();
  return m.client_setup_status_idle();
}

type ClientSetupCardProps = {
  savedAt: string;
  isDirty: boolean;
};

export function ClientSetupCard({ savedAt, isDirty }: ClientSetupCardProps) {
  const [previewState, setPreviewState] = useState<RequestState>("idle");
  const [previewMessage, setPreviewMessage] = useState("");
  const [setup, setSetup] = useState<ClientSetupInfo | null>(null);
  const [claudeAction, setClaudeAction] = useState<ActionState>(toActionState);
  const [codexAction, setCodexAction] = useState<ActionState>(toActionState);

  const canApply = useMemo(() => !isDirty, [isDirty]);
  const isWorking =
    previewState === "working" ||
    claudeAction.state === "working" ||
    codexAction.state === "working";

  const loadPreview = useCallback(async () => {
    setPreviewState("working");
    setPreviewMessage("");
    try {
      const result = await invoke<ClientSetupInfo>("preview_client_setup");
      setSetup(result);
      setPreviewState("success");
    } catch (error) {
      setPreviewState("error");
      setPreviewMessage(parseError(error));
    }
  }, []);

  useEffect(() => {
    void loadPreview();
  }, [loadPreview, savedAt]);

  const applyClaudeConfig = useCallback(async () => {
    setClaudeAction({ state: "working", message: "", lastPath: "" });
    try {
      const result = await invoke<ClientConfigWriteResult>("write_claude_code_settings");
      const path = result.paths.join(", ");
      setClaudeAction({
        state: "success",
        message: m.client_setup_apply_success({ path }),
        lastPath: path,
      });
      await loadPreview();
    } catch (error) {
      setClaudeAction({ state: "error", message: parseError(error), lastPath: "" });
    }
  }, [loadPreview]);

  const applyCodexConfig = useCallback(async () => {
    setCodexAction({ state: "working", message: "", lastPath: "" });
    try {
      const result = await invoke<ClientConfigWriteResult>("write_codex_config");
      const path = result.paths.join(", ");
      setCodexAction({
        state: "success",
        message: m.client_setup_apply_success({ path }),
        lastPath: path,
      });
      await loadPreview();
    } catch (error) {
      setCodexAction({ state: "error", message: parseError(error), lastPath: "" });
    }
  }, [loadPreview]);

  return (
    <Card data-slot="client-setup-card">
      <CardHeader>
        <CardTitle>{m.client_setup_title()}</CardTitle>
        <CardDescription>{m.client_setup_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={toBadgeVariant(previewState)}>{toBadgeLabel(previewState)}</Badge>
          <Button type="button" variant="outline" size="sm" onClick={loadPreview} disabled={isWorking}>
            {m.common_refresh()}
          </Button>
        </div>

        {previewMessage ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {previewMessage}
          </div>
        ) : null}

        {isDirty ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {m.client_setup_dirty_notice()}
          </div>
        ) : null}

        {setup ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3">
            <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
              {m.client_setup_proxy_base_url_label()}
            </p>
            <p className="mt-1 font-mono text-xs text-foreground/80">
              {setup.proxy_http_base_url}
            </p>
          </div>
        ) : null}

        <Separator />

        <div className="grid gap-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium text-foreground">{m.client_setup_claude_title()}</p>
              <p className="mt-1 text-xs text-muted-foreground">
                {m.client_setup_claude_desc()}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Badge variant={toBadgeVariant(claudeAction.state)}>{toBadgeLabel(claudeAction.state)}</Badge>
              <Button type="button" onClick={applyClaudeConfig} disabled={!canApply || isWorking || !setup}>
                {m.client_setup_apply()}
              </Button>
            </div>
          </div>

          {setup ? (
            <div className="space-y-2 rounded-md border border-border/60 bg-background/60 p-3">
              <div className="space-y-1">
                <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
                  {m.client_setup_target_file_label()}
                </p>
                <p className="font-mono text-xs text-foreground/80">{setup.claude_settings_path}</p>
              </div>
              <div className="space-y-1">
                <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
                  {m.client_setup_effective_env_label()}
                </p>
                <div className="space-y-1 font-mono text-xs text-foreground/80">
                  <div>ANTHROPIC_BASE_URL={setup.claude_base_url}</div>
                  <div>
                    ANTHROPIC_AUTH_TOKEN=
                    {setup.claude_auth_token_configured ? "••••••••" : m.client_setup_not_configured()}
                  </div>
                </div>
              </div>
              <p className="text-xs text-muted-foreground">{m.client_setup_backup_hint()}</p>
            </div>
          ) : null}

          {claudeAction.message ? (
            <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
              {claudeAction.message}
            </div>
          ) : null}

          <Separator />

          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium text-foreground">{m.client_setup_codex_title()}</p>
              <p className="mt-1 text-xs text-muted-foreground">
                {m.client_setup_codex_desc()}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Badge variant={toBadgeVariant(codexAction.state)}>{toBadgeLabel(codexAction.state)}</Badge>
              <Button type="button" onClick={applyCodexConfig} disabled={!canApply || isWorking || !setup}>
                {m.client_setup_apply()}
              </Button>
            </div>
          </div>

          {setup ? (
            <div className="space-y-2 rounded-md border border-border/60 bg-background/60 p-3">
              <div className="space-y-1">
                <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
                  {m.client_setup_target_file_label()}
                </p>
                <p className="font-mono text-xs text-foreground/80">{setup.codex_config_path}</p>
                <p className="font-mono text-xs text-foreground/80">{setup.codex_auth_path}</p>
              </div>
              <div className="space-y-1">
                <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
                  {m.client_setup_effective_config_label()}
                </p>
                <div className="space-y-1 font-mono text-xs text-foreground/80">
                  <div>disable_response_storage = {setup.codex_disable_response_storage ? "true" : "false"}</div>
                  <div>model = "{setup.codex_model}"</div>
                  <div>model_provider = "{setup.codex_model_provider}"</div>
                  <div>model_reasoning_effort = "{setup.codex_model_reasoning_effort}"</div>
                  <div>network_access = "{setup.codex_network_access}"</div>
                  <div>preferred_auth_method = "{setup.codex_preferred_auth_method}"</div>
                  <div>[model_providers.{setup.codex_model_provider}]</div>
                  <div>base_url = "{setup.codex_provider_base_url}"</div>
                  <div>name = "{setup.codex_provider_name}"</div>
                  <div>
                    requires_openai_auth = {setup.codex_provider_requires_openai_auth ? "true" : "false"}
                  </div>
                  <div>wire_api = "{setup.codex_provider_wire_api}"</div>
                  <div>
                    {`auth.json: OPENAI_API_KEY = "${
                      setup.codex_api_key_configured
                        ? "••••••••"
                        : m.client_setup_not_configured()
                    }"`}
                  </div>
                </div>
              </div>
              <p className="text-xs text-muted-foreground">{m.client_setup_backup_hint()}</p>
            </div>
          ) : null}

          {codexAction.message ? (
            <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
              {codexAction.message}
            </div>
          ) : null}

          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {m.client_setup_plaintext_warning()}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
