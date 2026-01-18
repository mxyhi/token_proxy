import { onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { Outlet } from "@tanstack/react-router";
import { useEffect } from "react";

import { handleKiroCallback } from "@/features/kiro/api";
import { UpdateNotifier } from "@/features/update/UpdateNotifier";
import { UpdaterProvider } from "@/features/update/updater";

import "./App.css";

function App() {
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    onOpenUrl((urls) => {
      for (const url of urls) {
        if (url.startsWith("kiro://")) {
          void handleKiroCallback(url);
        }
      }
    })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {});
    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, []);
  return (
    <UpdaterProvider>
      <UpdateNotifier />
      <main className="app-shell">
        <div data-slot="app-shell" className="relative z-10 h-full min-h-0">
          <Outlet />
        </div>
      </main>
    </UpdaterProvider>
  );
}

export default App;
