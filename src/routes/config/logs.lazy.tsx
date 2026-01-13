import { createLazyFileRoute } from "@tanstack/react-router";

import { ConfigRoutePage } from "@/features/config/ConfigRoutePage";

export const Route = createLazyFileRoute("/config/logs")({
  component: ConfigLogsRoute,
});

function ConfigLogsRoute() {
  return <ConfigRoutePage sectionId="logs" />;
}
