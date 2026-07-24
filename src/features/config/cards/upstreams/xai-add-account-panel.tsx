import { useState } from "react";

import { Button } from "@/components/ui/button";
import { m } from "@/paraglide/messages.js";

type XaiImportMode = "login" | "refresh_token" | "json" | "file";

type XaiAddAccountPanelProps = {
  busy: boolean;
  statusText: string;
  verificationUrl: string;
  userCode: string;
  onLogin: () => Promise<void>;
  onImportRefreshTokens: (contents: string) => Promise<void>;
  onImportText: (contents: string) => Promise<void>;
  onImportFile: () => Promise<void>;
  onImportDirectory: () => Promise<void>;
};

function countInputLines(value: string) {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean).length;
}

function modeLabel(mode: XaiImportMode) {
  if (mode === "refresh_token") {
    return m.xai_manual_mode_refresh_token();
  }
  if (mode === "json") {
    return m.xai_manual_mode_json();
  }
  if (mode === "file") {
    return m.xai_manual_mode_file();
  }
  return m.xai_manual_mode_login();
}

function XaiDeviceCodeHint({
  verificationUrl,
  userCode,
}: {
  verificationUrl: string;
  userCode: string;
}) {
  if (!verificationUrl || !userCode) {
    return null;
  }
  return (
    <div className="rounded-md border border-border/60 bg-background/70 p-3 text-xs">
      <p className="font-medium text-foreground">{m.xai_device_code_title()}</p>
      <p className="mt-2 break-all text-muted-foreground">{verificationUrl}</p>
      <p className="mt-1 font-mono text-sm text-foreground">{userCode}</p>
      <p className="mt-2 text-muted-foreground">{m.xai_login_open_hint()}</p>
    </div>
  );
}

export function XaiAddAccountPanel({
  busy,
  statusText,
  verificationUrl,
  userCode,
  onLogin,
  onImportRefreshTokens,
  onImportText,
  onImportFile,
  onImportDirectory,
}: XaiAddAccountPanelProps) {
  const [mode, setMode] = useState<XaiImportMode>("login");
  const [manualInput, setManualInput] = useState("");
  const lineCount = countInputLines(manualInput);

  const switchMode = (nextMode: XaiImportMode) => {
    setMode(nextMode);
    setManualInput("");
  };

  const submitManualInput = async () => {
    const contents = manualInput.trim();
    if (!contents) {
      return;
    }
    // 凭证不可进入日志，只记录入口和输入规模用于排查交互问题。
    console.debug("[providers-xai-import] manual import submitted", {
      mode,
      lines: lineCount,
      length: contents.length,
    });
    if (mode === "refresh_token") {
      await onImportRefreshTokens(contents);
    } else {
      await onImportText(contents);
    }
    setManualInput("");
  };

  return (
    <div
      data-slot="providers-add-panel-xai"
      className="space-y-2 rounded-md border border-border/60 bg-muted/20 p-3"
    >
      <div className="inline-flex flex-wrap rounded-lg border border-border/60 bg-background/70 p-1">
        {(["login", "refresh_token", "json", "file"] as const).map((nextMode) => (
          <Button
            key={nextMode}
            type="button"
            size="sm"
            variant={mode === nextMode ? "default" : "ghost"}
            onClick={() => switchMode(nextMode)}
            disabled={busy}
            data-slot={`providers-add-xai-mode-${nextMode}`}
          >
            {modeLabel(nextMode)}
          </Button>
        ))}
      </div>

      {mode === "login" ? (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => void onLogin()}
          disabled={busy}
          data-slot="providers-add-xai-login"
        >
          {m.xai_login_button()}
        </Button>
      ) : null}

      {mode === "refresh_token" || mode === "json" ? (
        <div className="space-y-2">
          <div>
            <label className="text-xs font-medium text-foreground">{modeLabel(mode)}</label>
            <p className="mt-1 text-xs text-muted-foreground">
              {mode === "refresh_token"
                ? m.xai_manual_refresh_token_desc()
                : m.xai_manual_json_desc()}
            </p>
          </div>
          <textarea
            value={manualInput}
            onChange={(event) => setManualInput(event.target.value)}
            placeholder={
              mode === "refresh_token"
                ? m.xai_manual_refresh_token_placeholder()
                : m.xai_manual_json_placeholder()
            }
            spellCheck={false}
            rows={mode === "json" ? 8 : 4}
            className="border-input bg-background placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-ring/50 min-h-24 w-full resize-y rounded-md border px-3 py-2 font-mono text-sm shadow-xs outline-none focus-visible:ring-[3px]"
            data-slot="providers-add-xai-manual-input"
          />
          {lineCount > 1 ? (
            <p className="text-xs text-muted-foreground">
              {m.xai_manual_input_count({ count: lineCount })}
            </p>
          ) : null}
          <Button
            type="button"
            variant="secondary"
            size="sm"
            onClick={() => void submitManualInput()}
            disabled={busy || !manualInput.trim()}
            data-slot="providers-add-xai-manual-submit"
          >
            {m.xai_manual_import_button()}
          </Button>
        </div>
      ) : null}

      {mode === "file" ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void onImportFile()}
            disabled={busy}
            data-slot="providers-add-xai-import-file"
          >
            {m.xai_import_file_button()}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void onImportDirectory()}
            disabled={busy}
            data-slot="providers-add-xai-import-directory"
          >
            {m.xai_import_directory_button()}
          </Button>
        </div>
      ) : null}

      {statusText ? <p className="text-xs text-muted-foreground">{statusText}</p> : null}
      <XaiDeviceCodeHint verificationUrl={verificationUrl} userCode={userCode} />
    </div>
  );
}
