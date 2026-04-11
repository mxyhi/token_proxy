import { AppShell } from "@/layouts/app-shell";
import { DashboardPanel } from "@/features/dashboard/DashboardPanel";
import { m } from "@/paraglide/messages.js";

export function DashboardPage() {
  return (
    <AppShell title={m.config_section_dashboard_label()}>
      <DashboardPanel />
    </AppShell>
  );
}
