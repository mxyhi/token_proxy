import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import type { ConfigForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type ValidationCardProps = {
  form: ConfigForm;
  validation: { valid: boolean; message: string };
};

export function ValidationCard({ form, validation }: ValidationCardProps) {
  const hasEnabledUpstreams = form.upstreams.some((upstream) => upstream.enabled);
  const upstreamBadgeVariant = hasEnabledUpstreams ? "default" : "secondary";
  const upstreamBadgeLabel = hasEnabledUpstreams ? m.validation_ready() : m.validation_disabled();
  return (
    <Card data-slot="validation-card">
      <CardHeader>
        <CardTitle>{m.validation_title()}</CardTitle>
        <CardDescription>{m.validation_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3 text-sm">
        <div className="flex items-center justify-between">
          <span>{m.validation_host_row()}</span>
          <Badge variant={form.host.trim() ? "default" : "destructive"}>
            {form.host.trim() ? m.validation_ready() : m.validation_missing()}
          </Badge>
        </div>
        <div className="flex items-center justify-between">
          <span>{m.validation_port_row()}</span>
          <Badge variant={validation.valid ? "default" : "secondary"}>
            {validation.valid ? m.validation_ok() : m.validation_check()}
          </Badge>
        </div>
        <div className="flex items-center justify-between">
          <span>{m.validation_upstreams_row()}</span>
          <Badge variant={upstreamBadgeVariant}>{upstreamBadgeLabel}</Badge>
        </div>
      </CardContent>
    </Card>
  );
}
