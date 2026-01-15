import { Info } from "lucide-react";

import { Switch } from "@/components/ui/switch";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PasswordInput } from "@/components/ui/password-input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { ProxyServicePanel, type ProxyServiceViewProps } from "@/features/config/cards/proxy-service-card";
import { type ConfigForm, LOG_LEVELS } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type ProxyCoreCardProps = {
  form: ConfigForm;
  showLocalKey: boolean;
  onToggleLocalKey: () => void;
  onChange: (patch: Partial<ConfigForm>) => void;
  proxyService: ProxyServiceViewProps;
};

type ProxyCoreFieldsProps = Pick<
  ProxyCoreCardProps,
  "form" | "showLocalKey" | "onToggleLocalKey" | "onChange"
>;

function ProxyCoreFields({ form, showLocalKey, onToggleLocalKey, onChange }: ProxyCoreFieldsProps) {
  return (
    <>
      <div className="grid gap-4 sm:grid-cols-2">
        <div className="grid gap-2">
          <Label htmlFor="proxy-host">{m.proxy_core_host_label()}</Label>
          <Input
            id="proxy-host"
            value={form.host}
            onChange={(event) => onChange({ host: event.target.value })}
            placeholder="127.0.0.1"
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="proxy-port">{m.proxy_core_port_label()}</Label>
          <Input
            id="proxy-port"
            value={form.port}
            onChange={(event) => onChange({ port: event.target.value })}
            placeholder="9208"
            inputMode="numeric"
          />
        </div>
      </div>
      <div className="grid gap-2">
        <Label htmlFor="proxy-key">{m.proxy_core_local_api_key_label()}</Label>
        <PasswordInput
          id="proxy-key"
          visible={showLocalKey}
          onVisibilityChange={onToggleLocalKey}
          value={form.localApiKey}
          onChange={(event) => onChange({ localApiKey: event.target.value })}
          placeholder={m.common_optional()}
        />
        <p className="text-xs text-muted-foreground">{m.proxy_core_local_api_key_help()}</p>
      </div>
      <div className="grid gap-2">
        <Label htmlFor="app-proxy-url">{m.proxy_core_app_proxy_url_label()}</Label>
        <Input
          id="app-proxy-url"
          value={form.appProxyUrl}
          onChange={(event) => onChange({ appProxyUrl: event.target.value })}
          placeholder="socks5h://127.0.0.1:7891"
        />
        <p className="text-xs text-muted-foreground">
          {m.proxy_core_app_proxy_url_help({ placeholder: "$app_proxy_url" })}
        </p>
      </div>
      <div className="grid gap-2">
        <div className="flex items-center gap-2">
          <Label htmlFor="proxy-log-level">{m.proxy_core_log_level_label()}</Label>
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                className="inline-flex size-5 items-center justify-center rounded-full text-muted-foreground transition hover:text-foreground"
                aria-label={m.proxy_core_log_level_help()}
              >
                <Info className="size-3.5" aria-hidden="true" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="top" align="start">
              {m.proxy_core_log_level_help()}
            </TooltipContent>
          </Tooltip>
        </div>
        <Select
          value={form.logLevel}
          onValueChange={(nextValue) => {
            const nextLevel = toLogLevel(nextValue);
            if (nextLevel) {
              onChange({ logLevel: nextLevel });
            }
          }}
        >
          <SelectTrigger id="proxy-log-level">
            <SelectValue placeholder={m.proxy_core_log_level_placeholder()} />
          </SelectTrigger>
          <SelectContent>
            {LOG_LEVELS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label()}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </>
  );
}

const LOG_LEVEL_VALUES: ReadonlySet<string> = new Set(LOG_LEVELS.map((level) => level.value));

function toLogLevel(value: string): ConfigForm["logLevel"] | null {
  return LOG_LEVEL_VALUES.has(value) ? (value as ConfigForm["logLevel"]) : null;
}

type ProxyCoreFormatConversionProps = {
  enabled: boolean;
  onToggle: (value: boolean) => void;
};

function ProxyCoreFormatConversion({ enabled, onToggle }: ProxyCoreFormatConversionProps) {
  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-3">
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1">
          <p className="text-sm font-medium text-foreground">{m.proxy_core_format_conversion_title()}</p>
          <p className="text-xs text-muted-foreground">
            {m.proxy_core_format_conversion_desc({
              chat: "/v1/chat/completions",
              responses: "/v1/responses",
              messages: "/v1/messages",
            })}
          </p>
        </div>
        <Switch
          id="enable-format-conversion"
          checked={enabled}
          onCheckedChange={onToggle}
          aria-label={m.proxy_core_format_conversion_aria()}
        />
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {m.proxy_core_format_conversion_default_disabled()}
      </p>
    </div>
  );
}

type ProxyCoreServiceSectionProps = {
  proxyService: ProxyServiceViewProps;
};

function ProxyCoreServiceSection({ proxyService }: ProxyCoreServiceSectionProps) {
  return (
    <div className="space-y-4">
      <div className="rounded-lg border border-border/60 bg-background/60 p-4">
        <ProxyServicePanel {...proxyService} />
      </div>
    </div>
  );
}

export function ProxyCoreCard({
  form,
  showLocalKey,
  onToggleLocalKey,
  onChange,
  proxyService,
}: ProxyCoreCardProps) {
  return (
    <Card data-slot="proxy-core-card">
      <CardHeader>
        <CardTitle>{m.proxy_core_title()}</CardTitle>
        <CardDescription>{m.proxy_core_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        <ProxyCoreFields
          form={form}
          showLocalKey={showLocalKey}
          onToggleLocalKey={onToggleLocalKey}
          onChange={onChange}
        />
        <ProxyCoreFormatConversion
          enabled={form.enableApiFormatConversion}
          onToggle={(checked) => onChange({ enableApiFormatConversion: checked })}
        />
        <ProxyCoreServiceSection proxyService={proxyService} />
      </CardContent>
    </Card>
  );
}
