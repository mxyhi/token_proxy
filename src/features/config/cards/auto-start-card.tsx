import { Switch } from "@/components/ui/switch";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { m } from "@/paraglide/messages.js";

type AutoStartStatus = "idle" | "loading" | "error";

type AutoStartCardProps = {
  enabled: boolean;
  status: AutoStartStatus;
  message: string;
  onChange: (value: boolean) => void;
};

export function AutoStartCard({
  enabled,
  status,
  message,
  onChange,
}: AutoStartCardProps) {
  const isLoading = status === "loading";
  const isError = status === "error";
  const errorText = m.auto_start_status_error({
    message: message || m.common_unknown(),
  });

  return (
    <Card data-slot="auto-start-card">
      <CardHeader>
        <CardTitle>{m.auto_start_title()}</CardTitle>
        <CardDescription>{m.auto_start_desc()}</CardDescription>
        <CardAction>
          <Switch
            checked={enabled}
            onCheckedChange={onChange}
            disabled={isLoading || isError}
            aria-label={m.auto_start_aria()}
          />
        </CardAction>
      </CardHeader>
      <CardContent className="space-y-2 text-xs text-muted-foreground">
        <p>{m.auto_start_hint()}</p>
        {isLoading ? <p>{m.auto_start_status_loading()}</p> : null}
        {isError ? <p className="text-destructive">{errorText}</p> : null}
      </CardContent>
    </Card>
  );
}
