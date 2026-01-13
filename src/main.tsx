import React from "react";
import ReactDOM from "react-dom/client";
import { RouterProvider, createRouter } from "@tanstack/react-router";
import { ThemeProvider } from "next-themes";
import { routeTree } from "./routeTree.gen";

import { I18nProvider } from "@/lib/i18n";
import { LanguageObserver } from "@/components/LanguageObserver";
import { Toaster } from "@/components/ui/sonner";

const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      {/* Follow system theme and persist to localStorage; class drives dark styles. */}
      <ThemeProvider
        attribute="class"
        defaultTheme="system"
        enableSystem
        storageKey="token-proxy-theme"
        disableTransitionOnChange
      >
        <RouterProvider router={router} />
        <Toaster position="bottom-right" closeButton richColors />
        {/* Isolated language subscription - prevents global re-renders when language changes */}
        <LanguageObserver />
      </ThemeProvider>
    </I18nProvider>
  </React.StrictMode>
);
