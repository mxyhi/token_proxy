import { Switch } from "@/components/ui/switch";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  type ConfigForm,
  type TrayTokenRateFormat,
  TRAY_TOKEN_RATE_FORMATS,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

const TRAY_TOKEN_RATE_FORMAT_VALUES: ReadonlySet<string> = new Set(
  TRAY_TOKEN_RATE_FORMATS.map((format) => format.value)
);

function toTrayTokenRateFormat(value: string): TrayTokenRateFormat | null {
  return TRAY_TOKEN_RATE_FORMAT_VALUES.has(value)
    ? (value as TrayTokenRateFormat)
    : null;
}

type TrayTokenRateCardProps = {
  value: ConfigForm["trayTokenRate"];
  onChange: (value: ConfigForm["trayTokenRate"]) => void;
};

export function TrayTokenRateCard({ value, onChange }: TrayTokenRateCardProps) {
  return (
    <Card data-slot="tray-token-rate-card">
      <CardHeader>
        <CardTitle>{m.proxy_core_tray_token_rate_title()}</CardTitle>
        <CardDescription>{m.proxy_core_tray_token_rate_desc()}</CardDescription>
        <CardAction>
          <Switch
            checked={value.enabled}
            onCheckedChange={(checked) => onChange({ ...value, enabled: checked })}
            aria-label={m.proxy_core_tray_token_rate_aria()}
          />
        </CardAction>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid gap-2">
          <Label htmlFor="tray-token-rate-format">
            {m.proxy_core_tray_token_rate_format_label()}
          </Label>
          <Select
            value={value.format}
            onValueChange={(nextValue) => {
              const nextFormat = toTrayTokenRateFormat(nextValue);
              if (nextFormat) {
                onChange({ ...value, format: nextFormat });
              }
            }}
            disabled={!value.enabled}
          >
            <SelectTrigger id="tray-token-rate-format">
              <SelectValue placeholder={m.proxy_core_tray_token_rate_format_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {TRAY_TOKEN_RATE_FORMATS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className="text-xs text-muted-foreground">
            {m.proxy_core_tray_token_rate_macos_only()}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
