import { Link } from "@tanstack/react-router";
import { RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { KiroAccountSummary } from "@/features/kiro/types";
import { getSectionRoute } from "@/features/config/sections";
import { m } from "@/paraglide/messages.js";

type KiroAccountSelectProps = {
  accountId: string;
  accounts: KiroAccountSummary[];
  loading: boolean;
  error: string;
  onRefresh: () => void;
  onSelect: (accountId: string) => void;
};

function formatAccountLabel(account: KiroAccountSummary) {
  return account.account_id;
}

function formatAccountStatus(account: KiroAccountSummary) {
  return account.status === "expired"
    ? m.kiro_account_status_expired()
    : m.kiro_account_status_active();
}

export function KiroAccountSelect({
  accountId,
  accounts,
  loading,
  error,
  onRefresh,
  onSelect,
}: KiroAccountSelectProps) {
  return (
    <div data-slot="kiro-account-select" className="contents">
      <Label>{m.field_kiro_account()}</Label>
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <Select value={accountId.trim() ? accountId : undefined} onValueChange={onSelect}>
            <SelectTrigger className="flex-1">
              <SelectValue placeholder={m.kiro_account_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {accounts.map((account) => (
                <SelectItem key={account.account_id} value={account.account_id}>
                  {formatAccountLabel(account)} · {formatAccountStatus(account)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            type="button"
            size="icon"
            variant="outline"
            onClick={onRefresh}
            disabled={loading}
            aria-label={m.common_refresh()}
          >
            <RefreshCw
              className={["size-4", loading ? "animate-spin" : ""].filter(Boolean).join(" ")}
              aria-hidden="true"
            />
          </Button>
        </div>
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <Link className="text-primary hover:underline" to={getSectionRoute("providers")}>
            {m.kiro_account_manage()}
          </Link>
        </div>
        {error ? <p className="text-xs text-destructive">{error}</p> : null}
      </div>
    </div>
  );
}
