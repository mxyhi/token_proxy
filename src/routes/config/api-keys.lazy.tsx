import { createLazyFileRoute } from "@tanstack/react-router";
import { ConfigScreen } from "@/features/config/ConfigScreen";

export const Route = createLazyFileRoute("/config/api-keys")({
  component: () => <ConfigScreen activeSectionId="api-keys" />,
});
