import { AppShell } from "@/layouts/app-shell";
import { LogsPanel } from "@/features/logs/LogsPanel";
import { m } from "@/paraglide/messages.js";

export function LogsPage() {
  return (
    <AppShell title={m.config_section_logs_label()}>
      <LogsPanel />
    </AppShell>
  );
}
