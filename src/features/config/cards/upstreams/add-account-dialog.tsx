/**
 * Upstream 页「添加账户」弹窗：Kiro / Codex / xAI 登录与导入。
 * 成功后由父级 reload config，让后端 reconcile 产出 account-backed Upstream。
 */
import { useState } from "react";

import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useCodexLogin } from "@/features/codex/use-codex-login";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroLogin } from "@/features/kiro/use-kiro-login";
import type { KiroLoginMethod } from "@/features/kiro/types";
import { XaiAddAccountPanel } from "@/features/config/cards/upstreams/xai-add-account-panel";
import { useXaiAccounts } from "@/features/xai/use-xai-accounts";
import { useXaiLogin } from "@/features/xai/use-xai-login";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type AddDialogProvider = "kiro" | "codex" | "xai";
type CodexManualInputMode =
  | "login"
  | "refresh_token"
  | "mobile_refresh_token"
  | "codex_session"
  | "file";

type AddAccountDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 登录/导入成功后刷新配置，使 account_upstreams reconcile 生效。 */
  onAccountsChanged: () => Promise<void> | void;
};

function countInputLines(value: string) {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean).length;
}

function codexModeLabel(mode: CodexManualInputMode) {
  if (mode === "refresh_token") {
    return m.codex_manual_mode_refresh_token();
  }
  if (mode === "mobile_refresh_token") {
    return m.codex_manual_mode_mobile_refresh_token();
  }
  if (mode === "codex_session") {
    return m.codex_manual_mode_json_access_token();
  }
  if (mode === "file") {
    return m.codex_manual_mode_file();
  }
  return m.codex_manual_mode_login();
}

function codexManualPlaceholder(mode: CodexManualInputMode) {
  if (mode === "codex_session") {
    return m.codex_manual_json_placeholder();
  }
  if (mode === "mobile_refresh_token") {
    return m.codex_manual_mobile_refresh_token_placeholder();
  }
  return m.codex_manual_refresh_token_placeholder();
}

function codexManualDescription(mode: CodexManualInputMode) {
  if (mode === "codex_session") {
    return m.codex_manual_json_desc();
  }
  if (mode === "mobile_refresh_token") {
    return m.codex_manual_mobile_refresh_token_desc();
  }
  return m.codex_manual_refresh_token_desc();
}

function KiroLoginHint({
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
      <p className="font-medium text-foreground">{m.kiro_device_code_title()}</p>
      <p className="mt-2 break-all text-muted-foreground">{verificationUrl}</p>
      <p className="mt-1 font-mono text-sm text-foreground">{userCode}</p>
      <p className="mt-2 text-muted-foreground">{m.kiro_login_open_hint()}</p>
    </div>
  );
}

function CodexLoginHint({ loginUrl }: { loginUrl: string }) {
  if (!loginUrl) {
    return null;
  }
  return (
    <div className="rounded-md border border-border/60 bg-background/70 p-3 text-xs">
      <p className="font-medium text-foreground">{m.codex_login_url_title()}</p>
      <p className="mt-2 break-all text-muted-foreground">{loginUrl}</p>
      <p className="mt-2 text-muted-foreground">{m.codex_login_open_hint()}</p>
    </div>
  );
}

export function AddAccountDialog({
  open,
  onOpenChange,
  onAccountsChanged,
}: AddAccountDialogProps) {
  const [activeProvider, setActiveProvider] = useState<AddDialogProvider>("kiro");
  const [codexMode, setCodexMode] = useState<CodexManualInputMode>("login");
  const [codexManualInput, setCodexManualInput] = useState("");

  const handleAccountsChanged = async () => {
    console.debug("[upstream-add-account] accounts changed, reloading config");
    await Promise.resolve(onAccountsChanged());
  };

  // 弹窗关闭时不预加载账户列表，仅在导入/登录时走后端命令。
  const kiroAccounts = useKiroAccounts({ autoLoad: false });
  const codexAccounts = useCodexAccounts({ autoLoad: false });
  const xaiAccounts = useXaiAccounts({ autoLoad: false });

  const kiroLogin = useKiroLogin({
    onRefresh: async () => {
      await handleAccountsChanged();
    },
  });
  const codexLogin = useCodexLogin({
    onRefresh: async () => {
      await handleAccountsChanged();
    },
  });
  const xaiLogin = useXaiLogin({
    onRefresh: async () => {
      await handleAccountsChanged();
    },
  });

  const kiroBusy =
    kiroAccounts.loading ||
    kiroLogin.login.status === "waiting" ||
    kiroLogin.login.status === "polling";
  const codexBusy =
    codexAccounts.loading ||
    codexLogin.login.status === "waiting" ||
    codexLogin.login.status === "polling";
  const xaiBusy =
    xaiAccounts.loading ||
    xaiLogin.login.status === "waiting" ||
    xaiLogin.login.status === "polling";

  const kiroStatusText =
    kiroLogin.login.error ||
    (kiroLogin.login.status === "waiting" || kiroLogin.login.status === "polling"
      ? m.kiro_login_waiting()
      : kiroAccounts.error);
  const codexStatusText =
    codexLogin.login.error ||
    (codexLogin.login.status === "waiting" || codexLogin.login.status === "polling"
      ? m.codex_login_waiting()
      : codexAccounts.error);
  const xaiStatusText =
    xaiLogin.login.error ||
    (xaiLogin.login.status === "waiting" || xaiLogin.login.status === "polling"
      ? m.xai_login_waiting()
      : xaiAccounts.error);

  const kiroVerificationUrl =
    kiroLogin.login.start?.verification_uri_complete?.trim() ||
    kiroLogin.login.start?.verification_uri?.trim() ||
    "";
  const kiroUserCode = kiroLogin.login.start?.user_code?.trim() || "";
  const codexLoginUrl = codexLogin.login.start?.login_url?.trim() || "";
  const xaiVerificationUrl =
    xaiLogin.login.start?.verification_uri_complete?.trim() ||
    xaiLogin.login.start?.verification_uri?.trim() ||
    "";
  const xaiUserCode = xaiLogin.login.start?.user_code?.trim() || "";

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) {
      // 关闭时丢弃进行中的设备授权轮次，���免晚到回调污染下次打开。
      kiroLogin.resetLogin();
      codexLogin.resetLogin();
      xaiLogin.resetLogin();
      setCodexMode("login");
      setCodexManualInput("");
    }
    onOpenChange(nextOpen);
  };

  const runKiroLogin = async (method: KiroLoginMethod) => {
    console.debug("[upstream-add-account] kiro login", { method });
    await kiroLogin.beginLogin(method);
  };

  const importKiroIde = async () => {
    try {
      const directory = await openFileDialog({ directory: true, multiple: false });
      if (!directory || Array.isArray(directory)) {
        return;
      }
      console.debug("[upstream-add-account] kiro import ide");
      await kiroAccounts.importIde(directory);
      toast.success(m.kiro_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importKiroKam = async () => {
    try {
      const path = await openFileDialog({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path || Array.isArray(path)) {
        return;
      }
      console.debug("[upstream-add-account] kiro import kam");
      await kiroAccounts.importKam(path);
      toast.success(m.kiro_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importCodexRefreshTokens = async (
    contents: string,
    clientKind: "codex" | "mobile",
  ) => {
    try {
      console.debug("[upstream-add-account] codex import refresh tokens", {
        clientKind,
        lines: countInputLines(contents),
      });
      await codexAccounts.importRefreshTokens(contents, clientKind);
      toast.success(m.codex_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importCodexText = async (contents: string) => {
    try {
      console.debug("[upstream-add-account] codex import text", {
        lines: countInputLines(contents),
      });
      await codexAccounts.importText(contents);
      toast.success(m.codex_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importCodexFile = async () => {
    try {
      const path = await openFileDialog({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path || Array.isArray(path)) {
        return;
      }
      console.debug("[upstream-add-account] codex import file");
      await codexAccounts.importFile(path);
      toast.success(m.codex_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importCodexDirectory = async () => {
    try {
      const directory = await openFileDialog({ directory: true, multiple: false });
      if (!directory || Array.isArray(directory)) {
        return;
      }
      // 目录导入走 file 命令（后端按路径解析）；失败时 toast。
      console.debug("[upstream-add-account] codex import directory path as file root");
      await codexAccounts.importFile(directory);
      toast.success(m.codex_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importXaiRefreshTokens = async (contents: string) => {
    try {
      console.debug("[upstream-add-account] xai import refresh tokens", {
        lines: countInputLines(contents),
      });
      await xaiAccounts.importRefreshTokens(contents);
      toast.success(m.xai_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importXaiText = async (contents: string) => {
    try {
      console.debug("[upstream-add-account] xai import text", {
        lines: countInputLines(contents),
      });
      await xaiAccounts.importText(contents);
      toast.success(m.xai_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importXaiFile = async () => {
    try {
      const path = await openFileDialog({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path || Array.isArray(path)) {
        return;
      }
      console.debug("[upstream-add-account] xai import file");
      await xaiAccounts.importFile(path);
      toast.success(m.xai_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const importXaiDirectory = async () => {
    try {
      const directory = await openFileDialog({ directory: true, multiple: false });
      if (!directory || Array.isArray(directory)) {
        return;
      }
      console.debug("[upstream-add-account] xai import directory path as file root");
      await xaiAccounts.importFile(directory);
      toast.success(m.xai_import_success());
      await handleAccountsChanged();
    } catch (error) {
      toast.error(parseError(error));
    }
  };

  const showCodexManual =
    codexMode === "refresh_token" ||
    codexMode === "mobile_refresh_token" ||
    codexMode === "codex_session";

  const submitCodexManual = async () => {
    const contents = codexManualInput.trim();
    if (!contents) {
      return;
    }
    if (codexMode === "refresh_token") {
      await importCodexRefreshTokens(contents, "codex");
    } else if (codexMode === "mobile_refresh_token") {
      await importCodexRefreshTokens(contents, "mobile");
    } else {
      await importCodexText(contents);
    }
    setCodexManualInput("");
  };

  return (
    <Dialog modal open={open} onOpenChange={handleOpenChange}>
      <DialogContent data-slot="upstream-add-account-dialog" aria-describedby={undefined}>
        <DialogHeader>
          <DialogTitle>{m.upstreams_add_account()}</DialogTitle>
        </DialogHeader>
        <DialogBody className="space-y-4">
          <div
            data-slot="upstream-add-provider-switch"
            className="inline-flex rounded-lg border border-border/60 bg-muted/30 p-1"
          >
            <Button
              type="button"
              size="sm"
              variant={activeProvider === "kiro" ? "default" : "ghost"}
              onClick={() => setActiveProvider("kiro")}
            >
              {m.providers_kiro_title()}
            </Button>
            <Button
              type="button"
              size="sm"
              variant={activeProvider === "codex" ? "default" : "ghost"}
              onClick={() => setActiveProvider("codex")}
            >
              {m.providers_codex_title()}
            </Button>
            <Button
              type="button"
              size="sm"
              variant={activeProvider === "xai" ? "default" : "ghost"}
              onClick={() => setActiveProvider("xai")}
            >
              {m.providers_xai_title()}
            </Button>
          </div>

          {activeProvider === "kiro" ? (
            <div
              data-slot="upstream-add-panel-kiro"
              className="space-y-2 rounded-md border border-border/60 bg-muted/20 p-3"
            >
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  disabled={kiroBusy}
                  onClick={() => {
                    void runKiroLogin("aws");
                  }}
                >
                  {m.kiro_login_method_aws()}
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  disabled={kiroBusy}
                  onClick={() => {
                    void runKiroLogin("aws_authcode");
                  }}
                >
                  {m.kiro_login_method_aws_authcode()}
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  disabled={kiroBusy}
                  onClick={() => {
                    void runKiroLogin("google");
                  }}
                >
                  {m.kiro_login_method_google()}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={kiroBusy}
                  onClick={() => {
                    void importKiroIde();
                  }}
                >
                  {m.kiro_login_method_import()}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={kiroBusy}
                  onClick={() => {
                    void importKiroKam();
                  }}
                >
                  {m.kiro_login_method_import_kam()}
                </Button>
              </div>
              {kiroStatusText ? (
                <p className="text-xs text-muted-foreground">{kiroStatusText}</p>
              ) : null}
              <KiroLoginHint verificationUrl={kiroVerificationUrl} userCode={kiroUserCode} />
            </div>
          ) : null}

          {activeProvider === "codex" ? (
            <div
              data-slot="upstream-add-panel-codex"
              className="space-y-2 rounded-md border border-border/60 bg-muted/20 p-3"
            >
              <div className="inline-flex flex-wrap rounded-lg border border-border/60 bg-background/70 p-1">
                {(
                  [
                    "login",
                    "refresh_token",
                    "mobile_refresh_token",
                    "codex_session",
                    "file",
                  ] as const
                ).map((mode) => (
                  <Button
                    key={mode}
                    type="button"
                    size="sm"
                    variant={codexMode === mode ? "default" : "ghost"}
                    disabled={codexBusy}
                    onClick={() => {
                      setCodexMode(mode);
                      setCodexManualInput("");
                    }}
                  >
                    {codexModeLabel(mode)}
                  </Button>
                ))}
              </div>
              {codexMode === "login" ? (
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  disabled={codexBusy}
                  onClick={() => {
                    void codexLogin.beginLogin();
                  }}
                >
                  {m.codex_login_button()}
                </Button>
              ) : null}
              {showCodexManual ? (
                <div className="space-y-2">
                  <div>
                    <label className="text-xs font-medium text-foreground">
                      {codexModeLabel(codexMode)}
                    </label>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {codexManualDescription(codexMode)}
                    </p>
                  </div>
                  <textarea
                    value={codexManualInput}
                    onChange={(event) => setCodexManualInput(event.target.value)}
                    placeholder={codexManualPlaceholder(codexMode)}
                    spellCheck={false}
                    rows={codexMode === "codex_session" ? 8 : 4}
                    className="border-input bg-background placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-ring/50 min-h-24 w-full resize-y rounded-md border px-3 py-2 font-mono text-sm shadow-xs outline-none focus-visible:ring-[3px]"
                  />
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    disabled={codexBusy || !codexManualInput.trim()}
                    onClick={() => {
                      void submitCodexManual();
                    }}
                  >
                    {m.codex_manual_import_button()}
                  </Button>
                </div>
              ) : null}
              {codexMode === "file" ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={codexBusy}
                    onClick={() => {
                      void importCodexFile();
                    }}
                  >
                    {m.codex_import_file_button()}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={codexBusy}
                    onClick={() => {
                      void importCodexDirectory();
                    }}
                  >
                    {m.codex_import_directory_button()}
                  </Button>
                </div>
              ) : null}
              {codexStatusText ? (
                <p className="text-xs text-muted-foreground">{codexStatusText}</p>
              ) : null}
              <CodexLoginHint loginUrl={codexLoginUrl} />
            </div>
          ) : null}

          {activeProvider === "xai" ? (
            <XaiAddAccountPanel
              busy={xaiBusy}
              statusText={xaiStatusText}
              verificationUrl={xaiVerificationUrl}
              userCode={xaiUserCode}
              onLogin={async () => {
                await xaiLogin.beginLogin();
              }}
              onImportRefreshTokens={importXaiRefreshTokens}
              onImportText={importXaiText}
              onImportFile={importXaiFile}
              onImportDirectory={importXaiDirectory}
            />
          ) : null}
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}
