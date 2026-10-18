import { createLazyFileRoute } from "@tanstack/react-router";
import { ConfigScreen } from "@/features/config/ConfigScreen";

export const Route = createLazyFileRoute("/config/codex-models")({
  component: () => <ConfigScreen activeSectionId="codex-models" />,
});
