import { createLazyFileRoute } from "@tanstack/react-router";

import { LogsPage } from "@/features/logs/pages/logs-page";

export const Route = createLazyFileRoute("/config/logs")({
  component: ConfigLogsRoute,
});

function ConfigLogsRoute() {
  return <LogsPage />;
}
