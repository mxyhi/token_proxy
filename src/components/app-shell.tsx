import type { CSSProperties, ReactNode } from "react";

import { AppSidebar } from "@/components/app-sidebar";
import { SiteHeader } from "@/components/site-header";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";

type AppShellProps = {
  title: string;
  children: ReactNode;
  actions?: ReactNode;
};

export function AppShell({ title, children, actions }: AppShellProps) {
  return (
    <SidebarProvider
      className="h-full"
      style={
        {
          "--sidebar-width": "calc(var(--spacing) * 48)",
          "--header-height": "calc(var(--spacing) * 12)",
        } as CSSProperties
      }
    >
      <AppSidebar />
      <SidebarInset className="min-h-0 md:m-0 md:ml-0 md:rounded-none md:shadow-none">
        <div className="flex flex-1 min-h-0 flex-col">
          <ScrollArea className="flex-1 min-h-0">
            <div className="@container/main flex flex-1 flex-col gap-1">
              <SiteHeader title={title} actions={actions} />
              <div className="flex flex-col gap-2.5 py-2.5 md:gap-3.5 md:py-3.5">
                {children}
              </div>
            </div>
          </ScrollArea>
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

