import type { ComponentProps, ReactNode } from "react"
import { Link, useRouterState } from "@tanstack/react-router"
import { motion, useReducedMotion } from "motion/react"

import {
  Sidebar,
  SidebarContent,
  SidebarHeader,
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar"
import { CONFIG_SECTIONS, DEFAULT_CONFIG_SECTION, getSectionRoute } from "@/features/config/sections"
import { cn } from "@/lib/utils"

/** motion 共享布局 id：同一 LayoutGroup 内激活条在菜单项之间滑动 */
export const SIDEBAR_ACTIVE_LAYOUT_ID = "sidebar-nav-active"

/** 菜单项激活态：底色交给 motion 指示条，文字/图标用主色 */
const navItemClassName = cn(
  "relative z-10 overflow-hidden transition-colors duration-200 ease-out",
  "hover:bg-sidebar-accent/70",
  "focus-visible:ring-2 focus-visible:ring-sidebar-primary/45 focus-visible:ring-offset-1 focus-visible:ring-offset-sidebar",
  "data-[active=true]:bg-transparent data-[active=true]:font-medium",
  "data-[active=true]:text-sidebar-primary data-[active=true]:hover:bg-transparent",
  "data-[active=true]:hover:text-sidebar-primary",
  "data-[active=true]:[&_svg]:text-sidebar-primary",
  "motion-reduce:transition-none"
)

/**
 * 激活项滑动高亮。依赖：
 * 1. AppShellProvider 让侧边栏实例不随路由卸载
 * 2. AppShellFrame 内 LayoutGroup 包住 AppSidebar
 * 3. 同一 layoutId 在不同菜单项间迁移
 */
function SidebarActiveIndicator({ active }: { active: boolean }) {
  const reduceMotion = useReducedMotion()

  if (!active) {
    return null
  }

  console.debug("[app-sidebar] active indicator on item", {
    layoutId: SIDEBAR_ACTIVE_LAYOUT_ID,
    reduceMotion: Boolean(reduceMotion),
  })

  return (
    <motion.span
      layoutId={SIDEBAR_ACTIVE_LAYOUT_ID}
      data-slot="sidebar-active-indicator"
      aria-hidden
      className={cn(
        "pointer-events-none absolute inset-0 z-0 rounded-md",
        "bg-sidebar-primary/15",
        "shadow-[inset_3px_0_0_0_var(--sidebar-primary)]"
      )}
      transition={
        reduceMotion
          ? { duration: 0 }
          : { type: "spring", stiffness: 420, damping: 34, mass: 0.7 }
      }
    />
  )
}

function NavLinkContent({
  active,
  icon,
  label,
}: {
  active: boolean
  icon: ReactNode
  label: ReactNode
}) {
  return (
    <>
      <SidebarActiveIndicator active={active} />
      {icon}
      <span className="relative z-10">{label}</span>
    </>
  )
}

export function AppSidebar({ ...props }: ComponentProps<typeof Sidebar>) {
  const pathname = useRouterState({ select: (state) => state.location.pathname })
  const appTitle = import.meta.env.DEV ? "Token Proxy (dev)" : "Token Proxy"

  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              asChild
              className="data-[slot=sidebar-menu-button]:!p-1.5"
            >
              <Link to={getSectionRoute(DEFAULT_CONFIG_SECTION)}>
                <span className="text-base font-semibold">{appTitle}</span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              {CONFIG_SECTIONS.map((section) => {
                const isActive = pathname === section.route
                const Icon = section.icon
                return (
                  <SidebarMenuItem key={section.id}>
                    <SidebarMenuButton
                      asChild
                      isActive={isActive}
                      tooltip={section.label()}
                      className={navItemClassName}
                    >
                      <Link to={section.route}>
                        <NavLinkContent
                          active={isActive}
                          icon={<Icon className="relative z-10" />}
                          label={section.label()}
                        />
                      </Link>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                )
              })}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
    </Sidebar>
  )
}
