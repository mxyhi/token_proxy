import type { LucideIcon } from "lucide-react";
import {
  LayoutDashboard,
  Server,
  Settings,
  Shuffle,
  SlidersHorizontal,
} from "lucide-react";

import { m } from "@/paraglide/messages.js";

export type ConfigSectionId =
  | "dashboard"
  | "core"
  | "strategy"
  | "upstreams"
  | "agents"
  | "settings";

export type ConfigSectionRoute = `/config/${ConfigSectionId}`;

export type ConfigSection = {
  id: ConfigSectionId;
  route: ConfigSectionRoute;
  label: () => string;
  description: () => string;
  icon: LucideIcon;
};

export const CONFIG_SECTIONS: readonly ConfigSection[] = [
  {
    id: "dashboard",
    route: "/config/dashboard",
    label: () => m.config_section_dashboard_label(),
    description: () => m.config_section_dashboard_desc(),
    icon: LayoutDashboard,
  },
  {
    id: "core",
    route: "/config/core",
    label: () => m.config_section_core_label(),
    description: () => m.config_section_core_desc(),
    icon: SlidersHorizontal,
  },
  {
    id: "strategy",
    route: "/config/strategy",
    label: () => m.config_section_strategy_label(),
    description: () => m.config_section_strategy_desc(),
    icon: Shuffle,
  },
  {
    id: "upstreams",
    route: "/config/upstreams",
    label: () => m.config_section_upstreams_label(),
    description: () => m.config_section_upstreams_desc(),
    icon: Server,
  },
  {
    id: "agents",
    route: "/config/agents",
    label: () => m.config_section_agents_label(),
    description: () => m.config_section_agents_desc(),
    icon: Shuffle,
  },
  {
    id: "settings",
    route: "/config/settings",
    label: () => m.config_section_settings_label(),
    description: () => m.config_section_settings_desc(),
    icon: Settings,
  },
] as const;

const CONFIG_SECTION_IDS: ReadonlySet<string> = new Set(
  CONFIG_SECTIONS.map((section) => section.id)
);

export const DEFAULT_CONFIG_SECTION: ConfigSectionId = "dashboard";

const CONFIG_SECTION_BY_ID: Record<ConfigSectionId, ConfigSection> = CONFIG_SECTIONS.reduce(
  (acc, section) => {
    acc[section.id] = section;
    return acc;
  },
  {} as Record<ConfigSectionId, ConfigSection>
);

export function isConfigSectionId(value: string): value is ConfigSectionId {
  return CONFIG_SECTION_IDS.has(value);
}

export function toConfigSectionId(value: string): ConfigSectionId | null {
  return isConfigSectionId(value) ? value : null;
}

export function getSection(sectionId: ConfigSectionId) {
  return CONFIG_SECTION_BY_ID[sectionId];
}

export function findSection(sectionId: ConfigSectionId) {
  return getSection(sectionId);
}

export function getSectionRoute(sectionId: ConfigSectionId) {
  return getSection(sectionId).route;
}
