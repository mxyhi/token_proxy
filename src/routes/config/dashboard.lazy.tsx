import { createLazyFileRoute } from "@tanstack/react-router";

import { DashboardPage } from "@/features/dashboard/pages/dashboard-page";

export const Route = createLazyFileRoute("/config/dashboard")({
  component: ConfigDashboardRoute,
});

function ConfigDashboardRoute() {
  return <DashboardPage />;
}
