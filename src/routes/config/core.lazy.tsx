import { createLazyFileRoute } from "@tanstack/react-router";

import { CorePage } from "@/features/config/pages/core-page";

export const Route = createLazyFileRoute("/config/core")({
  component: ConfigCoreRoute,
});

function ConfigCoreRoute() {
  return <CorePage />;
}
