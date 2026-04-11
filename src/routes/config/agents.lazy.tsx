import { createLazyFileRoute } from "@tanstack/react-router"

import { AgentsPage } from "@/features/config/pages/agents-page"

export const Route = createLazyFileRoute("/config/agents")({
  component: ConfigAgentsRoute,
})

function ConfigAgentsRoute() {
  return <AgentsPage />
}
