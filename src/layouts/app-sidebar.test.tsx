import { cleanup, render, screen } from "@testing-library/react";
import type { ComponentProps, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SidebarProvider } from "@/components/ui/sidebar";
import { AppSidebar, SIDEBAR_ACTIVE_LAYOUT_ID } from "@/layouts/app-sidebar";
import { m } from "@/paraglide/messages.js";
import { setLocale } from "@/paraglide/runtime.js";

const routerState = vi.hoisted(() => ({
  pathname: "/config/dashboard",
}));

type RouterState = {
  location: {
    pathname: string;
  };
};

type UseRouterStateArgs<T> = {
  select?: (state: RouterState) => T;
};

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: ComponentProps<"a"> & { to: string; children: ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
  useRouterState: <T,>({ select }: UseRouterStateArgs<T>) => {
    const state: RouterState = { location: { pathname: routerState.pathname } };
    return select ? select(state) : state;
  },
}));

describe("layouts/AppSidebar", () => {
  afterEach(() => {
    cleanup();
    setLocale("en", { reload: false });
    routerState.pathname = "/config/dashboard";
  });

  it("only renders config section menu items", () => {
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>
    );

    const hrefs = screen.getAllByRole("link").map((link) => link.getAttribute("href"));
    expect(hrefs).not.toContain("/agent-node");
    expect(screen.getByRole("link", { name: m.config_section_agents_label() })).toHaveAttribute(
      "href",
      "/config/agents"
    );
  });

  it("marks the active route and renders the motion layout indicator", () => {
    routerState.pathname = "/config/agents";

    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>
    );

    const agentsLink = screen.getByRole("link", { name: m.config_section_agents_label() });
    expect(agentsLink).toHaveAttribute("data-active", "true");

    const indicator = container.querySelector('[data-slot="sidebar-active-indicator"]');
    expect(indicator).not.toBeNull();
    // motion 会把 layoutId 落到 DOM；常量导出便于回归 layout 契约
    expect(SIDEBAR_ACTIVE_LAYOUT_ID).toBe("sidebar-nav-active");
  });
});
