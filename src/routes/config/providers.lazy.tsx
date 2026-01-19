import { createLazyFileRoute } from "@tanstack/react-router";

import { ConfigRoutePage } from "@/features/config/ConfigRoutePage";

export const Route = createLazyFileRoute("/config/providers")({
  component: () => <ConfigRoutePage sectionId="providers" />,
});
