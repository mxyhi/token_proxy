import { AppShell } from "@/layouts/app-shell";
import { ProvidersPanel } from "@/features/providers/ProvidersPanel";
import { m } from "@/paraglide/messages.js";

export function ProvidersPage() {
  return (
    <AppShell title={m.config_section_providers_label()}>
      <ProvidersPanel />
    </AppShell>
  );
}
