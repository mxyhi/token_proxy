import { Outlet, createFileRoute } from "@tanstack/react-router";

import { ConfigScreenProvider } from "@/features/config/ConfigScreen";

function ConfigLayoutRoute() {
  return (
    <ConfigScreenProvider>
      <Outlet />
    </ConfigScreenProvider>
  );
}

export const Route = createFileRoute("/config")({
  component: ConfigLayoutRoute,
});
