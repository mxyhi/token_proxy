import { createFileRoute, useLocation } from "@tanstack/react-router";

import { ConfigScreen } from "@/features/config/ConfigScreen";
import { getSectionIdFromPathname } from "@/features/config/sections";

function ConfigLayoutRoute() {
  const location = useLocation();
  const sectionId = getSectionIdFromPathname(location.pathname);
  return <ConfigScreen activeSectionId={sectionId} />;
}

export const Route = createFileRoute("/config")({
  component: ConfigLayoutRoute,
});
