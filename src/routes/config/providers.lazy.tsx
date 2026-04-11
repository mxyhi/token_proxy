import { createLazyFileRoute } from "@tanstack/react-router";

import { ProvidersPage } from "@/features/providers/pages/providers-page";

export const Route = createLazyFileRoute("/config/providers")({
  component: ProvidersPage,
});
