import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { m } from "@/paraglide/messages.js";

type ConfigFileCardProps = {
  configPath: string;
  savedAt: string;
  isDirty: boolean;
};

export function ConfigFileCard({
  configPath,
  savedAt,
  isDirty,
}: ConfigFileCardProps) {
  return (
    <Card data-slot="config-file-card">
      <CardHeader>
        <CardTitle>{m.config_file_title()}</CardTitle>
        <CardDescription>{m.config_file_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-1">
          <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
            {m.config_file_location_label()}
          </p>
          <p className="font-mono text-xs text-foreground/80">{configPath || "--"}</p>
        </div>
        <Separator />
        <div className="space-y-1 text-sm text-muted-foreground">
          <p>{m.config_file_help_1()}</p>
          <p>{m.config_file_help_2()}</p>
        </div>
        {savedAt ? (
          <div className="text-xs text-muted-foreground">
            {m.config_file_last_saved_at({ time: savedAt })}
          </div>
        ) : null}
        {isDirty ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {m.config_file_unsaved_notice()}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}
