import { Outlet, createFileRoute } from "@tanstack/react-router";

function ConfigLayoutRoute() {
  return <Outlet />;
}

export const Route = createFileRoute("/config")({
  component: ConfigLayoutRoute,
});
