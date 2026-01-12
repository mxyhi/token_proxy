import { createLazyFileRoute } from "@tanstack/react-router"

import { ConfigRoutePage } from "@/features/config/ConfigRoutePage"

export const Route = createLazyFileRoute("/config/agents")({
  component: ConfigAgentsRoute,
})

function ConfigAgentsRoute() {
  return <ConfigRoutePage sectionId="agents" />
}
