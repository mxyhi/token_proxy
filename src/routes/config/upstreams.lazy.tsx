import { createLazyFileRoute } from "@tanstack/react-router";

import { UpstreamsPage } from "@/features/config/pages/upstreams-page";

export const Route = createLazyFileRoute("/config/upstreams")({
  component: ConfigUpstreamsRoute,
});

function ConfigUpstreamsRoute() {
  return <UpstreamsPage />;
}
