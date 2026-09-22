import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  MAIN_WINDOW_VISIBLE_EVENT,
  __resetUpdateNotifierAutoCheckForTests,
  UpdateNotifier,
} from "@/features/update/UpdateNotifier";
import { UpdaterProvider, useUpdater } from "@/features/update/updater";

type TauriEventHandler = (event: { event: string; id: number; payload: unknown }) => void;

const { eventHandlers, listenMock, navigateMock } = vi.hoisted(() => ({
  eventHandlers: new Map<string, TauriEventHandler>(),
  listenMock: vi.fn<(event: string, handler: TauriEventHandler) => Promise<() => void>>(),
  navigateMock: vi.fn<() => Promise<void>>(),
}));

let consoleInfoMock: ReturnType<typeof vi.spyOn>;

vi.mock("@tauri-apps/api/event", () => ({
  listen: listenMock,
}));

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => navigateMock,
}));

vi.mock("sonner", () => {
  const toastMock = vi.fn<(...args: unknown[]) => string>().mockReturnValue("toast");
  return {
    toast: Object.assign(toastMock, {
      dismiss: vi.fn(),
      error: vi.fn(() => "error-toast"),
      loading: vi.fn(() => "loading-toast"),
    }),
  };
});

function UpdaterHarness({ manual }: { manual?: boolean }) {
  const { actions, state } = useUpdater();

  useEffect(() => {
    actions.setAppProxyUrl("http://127.0.0.1:7890");
  }, [actions]);

  return (
    <>
      <UpdateNotifier />
      <output data-testid="update-ready">{state.appProxyUrlReady ? "ready" : "pending"}</output>
      <output data-testid="update-status">{state.status}</output>
      {manual ? (
        <button
          type="button"
          onClick={() => {
            void actions.checkForUpdate({ source: "manual" });
          }}
        >
          check-now
        </button>
      ) : null}
    </>
  );
}

describe("update/UpdateNotifier", () => {
  beforeEach(() => {
    __resetUpdateNotifierAutoCheckForTests();
    eventHandlers.clear();
    consoleInfoMock = vi.spyOn(console, "info").mockImplementation(() => undefined);
    navigateMock.mockReset();
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "visible",
    });
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({ config: { app_proxy_url: null } });
    vi.mocked(check).mockReset();
    vi.mocked(check).mockResolvedValue(null);
    listenMock.mockReset();
    listenMock.mockImplementation(async (event, handler) => {
      eventHandlers.set(event, handler);
      return () => {
        eventHandlers.delete(event);
      };
    });
  });

  afterEach(() => {
    consoleInfoMock.mockRestore();
    cleanup();
  });

  // MY-URL-COMPOSE PATCH G5：自动更新检测已关闭——启动与窗口可见均不得发起检查。

  it("does not check for updates on startup", async () => {
    render(
      <UpdaterProvider>
        <UpdaterHarness />
      </UpdaterProvider>
    );

    await waitFor(() => {
      expect(screen.getByTestId("update-ready")).toHaveTextContent("ready");
    });
    expect(vi.mocked(check)).not.toHaveBeenCalled();
    expect(screen.getByTestId("update-status")).toHaveTextContent("idle");
  });

  it("does not check for updates when the main window becomes visible", async () => {
    render(
      <UpdaterProvider>
        <UpdaterHarness />
      </UpdaterProvider>
    );

    await waitFor(() => {
      expect(screen.getByTestId("update-ready")).toHaveTextContent("ready");
    });

    const handler = eventHandlers.get(MAIN_WINDOW_VISIBLE_EVENT);
    expect(handler).toBeDefined();
    handler?.({ event: MAIN_WINDOW_VISIBLE_EVENT, id: 1, payload: null });
    handler?.({ event: MAIN_WINDOW_VISIBLE_EVENT, id: 2, payload: null });

    // 让可见事件的异步处理跑完
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(vi.mocked(check)).not.toHaveBeenCalled();
  });

  it("still allows a manual update check", async () => {
    render(
      <UpdaterProvider>
        <UpdaterHarness manual />
      </UpdaterProvider>
    );

    await waitFor(() => {
      expect(screen.getByTestId("update-ready")).toHaveTextContent("ready");
    });

    screen.getByRole("button", { name: "check-now" }).click();

    await waitFor(() => {
      expect(vi.mocked(check)).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.getByTestId("update-status")).toHaveTextContent("uptodate");
    });
  });
});
