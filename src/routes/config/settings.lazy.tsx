import { createLazyFileRoute } from "@tanstack/react-router";

import { SettingsPage } from "@/features/config/pages/settings-page";

export const Route = createLazyFileRoute("/config/settings")({
  component: ConfigFileRoute,
});

function ConfigFileRoute() {
  return <SettingsPage />;
}
