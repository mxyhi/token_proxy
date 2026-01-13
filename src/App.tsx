import { Outlet } from "@tanstack/react-router";

import { UpdateNotifier } from "@/features/update/UpdateNotifier";
import { UpdaterProvider } from "@/features/update/updater";

import "./App.css";

function App() {
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
