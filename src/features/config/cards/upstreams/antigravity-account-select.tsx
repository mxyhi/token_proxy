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
import type { AntigravityAccountSummary } from "@/features/antigravity/types";
import { getSectionRoute } from "@/features/config/sections";
import { m } from "@/paraglide/messages.js";

type AntigravityAccountSelectProps = {
  accountId: string;
  accounts: AntigravityAccountSummary[];
  loading: boolean;
  error: string;
  onRefresh: () => void;
  onSelect: (accountId: string) => void;
};

function formatAccountLabel(account: AntigravityAccountSummary) {
  return account.email?.trim() ? account.email : account.account_id;
}

function formatAccountStatus(account: AntigravityAccountSummary) {
  return account.status === "expired"
    ? m.antigravity_account_status_expired()
    : m.antigravity_account_status_active();
}

export function AntigravityAccountSelect({
  accountId,
  accounts,
  loading,
  error,
  onRefresh,
  onSelect,
}: AntigravityAccountSelectProps) {
  return (
    <div data-slot="antigravity-account-select" className="contents">
      <Label>{m.field_antigravity_account()}</Label>
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <Select value={accountId.trim() ? accountId : undefined} onValueChange={onSelect}>
            <SelectTrigger className="flex-1">
              <SelectValue placeholder={m.antigravity_account_placeholder()} />
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
            {m.antigravity_account_manage()}
          </Link>
        </div>
        {error ? <p className="text-xs text-destructive">{error}</p> : null}
      </div>
    </div>
  );
}
