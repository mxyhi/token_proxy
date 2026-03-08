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
import { ProxyServicePanel, type ProxyServiceViewProps } from "@/features/config/cards/proxy-service-card";
import { type ConfigForm, type KiroPreferredEndpoint } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

const KIRO_ENDPOINT_OPTIONS: ReadonlyArray<{
  value: KiroPreferredEndpoint;
  label: () => string;
}> = [
  { value: "ide", label: () => m.kiro_preferred_endpoint_ide() },
  { value: "cli", label: () => m.kiro_preferred_endpoint_cli() },
];

function isKiroPreferredEndpoint(value: string): value is KiroPreferredEndpoint {
  return value === "ide" || value === "cli";
}

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

function ProxyCoreFields({
  form,
  showLocalKey,
  onToggleLocalKey,
  onChange,
}: ProxyCoreFieldsProps) {
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
        <Label htmlFor="kiro-preferred-endpoint">
          {m.proxy_core_kiro_preferred_endpoint_label()}
        </Label>
        <Select
          value={form.kiroPreferredEndpoint}
          onValueChange={(value) => {
            if (isKiroPreferredEndpoint(value)) {
              onChange({ kiroPreferredEndpoint: value });
            }
          }}
        >
          <SelectTrigger id="kiro-preferred-endpoint">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {KIRO_ENDPOINT_OPTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label()}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {m.proxy_core_kiro_preferred_endpoint_help()}
        </p>
      </div>
      <div className="grid gap-2">
        <Label htmlFor="retryable-failure-cooldown-secs">
          {m.proxy_core_retryable_failure_cooldown_secs_label()}
        </Label>
        <Input
          id="retryable-failure-cooldown-secs"
          value={form.retryableFailureCooldownSecs}
          onChange={(event) =>
            onChange({ retryableFailureCooldownSecs: event.target.value })
          }
          placeholder="15"
          inputMode="numeric"
        />
        <p className="text-xs text-muted-foreground">
          {m.proxy_core_retryable_failure_cooldown_secs_help()}
        </p>
      </div>
    </>
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
        <ProxyCoreServiceSection proxyService={proxyService} />
      </CardContent>
    </Card>
  );
}
