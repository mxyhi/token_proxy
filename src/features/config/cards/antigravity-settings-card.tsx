import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { ConfigForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type AntigravitySettingsCardProps = {
  form: ConfigForm;
  onChange: (patch: Partial<ConfigForm>) => void;
};

export function AntigravitySettingsCard({ form, onChange }: AntigravitySettingsCardProps) {
  return (
    <Card data-slot="antigravity-settings-card">
      <CardHeader>
        <CardTitle>{m.antigravity_settings_title()}</CardTitle>
        <CardDescription>{m.antigravity_settings_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid gap-2">
          <Label htmlFor="antigravity-ide-db-path">{m.antigravity_settings_db_path_label()}</Label>
          <Input
            id="antigravity-ide-db-path"
            value={form.antigravityIdeDbPath}
            onChange={(event) => onChange({ antigravityIdeDbPath: event.target.value })}
            placeholder="~/Library/Application Support/Antigravity/User/globalStorage/state.vscdb"
          />
          <p className="text-xs text-muted-foreground">{m.antigravity_settings_db_path_help()}</p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="antigravity-app-paths">{m.antigravity_settings_app_paths_label()}</Label>
          <Input
            id="antigravity-app-paths"
            value={form.antigravityAppPaths}
            onChange={(event) => onChange({ antigravityAppPaths: event.target.value })}
            placeholder="/Applications/Antigravity.app, ~/Applications/Antigravity.app"
          />
          <p className="text-xs text-muted-foreground">{m.antigravity_settings_app_paths_help()}</p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="antigravity-process-names">
            {m.antigravity_settings_process_names_label()}
          </Label>
          <Input
            id="antigravity-process-names"
            value={form.antigravityProcessNames}
            onChange={(event) => onChange({ antigravityProcessNames: event.target.value })}
            placeholder="com.google.antigravity, com.todesktop.230313mzl4w4u92"
          />
          <p className="text-xs text-muted-foreground">
            {m.antigravity_settings_process_names_help()}
          </p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="antigravity-user-agent">
            {m.antigravity_settings_user_agent_label()}
          </Label>
          <Input
            id="antigravity-user-agent"
            value={form.antigravityUserAgent}
            onChange={(event) => onChange({ antigravityUserAgent: event.target.value })}
            placeholder="antigravity/1.104.0"
          />
          <p className="text-xs text-muted-foreground">
            {m.antigravity_settings_user_agent_help()}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
