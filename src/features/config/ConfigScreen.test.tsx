import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConfigScreen } from "@/features/config/ConfigScreen";
import { EMPTY_FORM, toPayload } from "@/features/config/form";
import type { ConfigForm, ProxyServiceStatus } from "@/features/config/types";
import { I18nProvider } from "@/lib/i18n";

const { setAppProxyUrlMock } = vi.hoisted(() => ({
  setAppProxyUrlMock: vi.fn<(value: string) => void>(),
}));

vi.mock("@/features/update/updater", () => ({
  useUpdater: () => ({
    state: {
      status: "idle",
      statusMessage: "",
      lastCheckedAt: "",
      updateInfo: null,
      updateHandle: null,
      downloadState: { downloaded: 0, total: 0 },
      lastCheckSource: null,
      appProxyUrl: "",
      appProxyUrlReady: true,
    },
    actions: {
      setAppProxyUrl: setAppProxyUrlMock,
      checkForUpdate: async () => undefined,
      downloadAndInstall: async () => undefined,
      relaunchApp: async () => undefined,
    },
  }),
}));

vi.mock("@/features/config/AppView", () => ({
  AppView: ({
    form,
    isDirty,
    status,
    statusMessage,
    onFormChange,
  }: {
    form: ConfigForm;
    isDirty: boolean;
    status: "idle" | "loading" | "saving" | "saved" | "error";
    statusMessage: string;
    onFormChange: (patch: Partial<ConfigForm>) => void;
  }) => (
    <div>
      <label htmlFor="mock-host">host</label>
      <input
        id="mock-host"
        value={form.host}
        onChange={(event) => onFormChange({ host: event.target.value })}
      />
      <div data-testid="status">{status}</div>
      <div data-testid="dirty">{String(isDirty)}</div>
      <div data-testid="status-message">{statusMessage}</div>
    </div>
  ),
}));

const PROXY_STATUS: ProxyServiceStatus = {
  state: "running",
  addr: "127.0.0.1:9208",
  last_error: null,
};

describe("config/ConfigScreen auto save", () => {
  beforeEach(() => {
    setAppProxyUrlMock.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.mocked(invoke).mockReset();
  });

  async function waitForAutoSaveWindow() {
    await new Promise((resolve) => {
      window.setTimeout(resolve, 1200);
    });
  }

  it("auto saves only after edits settle", async () => {
    const invokeMock = vi.mocked(invoke);
    const config = { ...toPayload(EMPTY_FORM), host: "10.0.0.1" };

    invokeMock.mockImplementation(async (command, args) => {
      if (command === "read_proxy_config") {
        return { path: "/tmp/config.json", config };
      }
      if (command === "proxy_status") {
        return PROXY_STATUS;
      }
      if (command === "write_proxy_config") {
        return { ...PROXY_STATUS, args };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(
      <I18nProvider>
        <ConfigScreen activeSectionId="core" />
      </I18nProvider>
    );

    const hostInput = screen.getByLabelText("host");
    await waitFor(() => {
      expect(hostInput).toHaveValue("10.0.0.1");
    });

    fireEvent.change(hostInput, { target: { value: "10.0.0.2" } });
    fireEvent.change(hostInput, { target: { value: "10.0.0.3" } });

    expect(
      invokeMock.mock.calls.filter(([command]) => command === "write_proxy_config")
    ).toHaveLength(0);

    await waitForAutoSaveWindow();

    await waitFor(() => {
      const writeCalls = invokeMock.mock.calls.filter(([command]) => command === "write_proxy_config");
      expect(writeCalls).toHaveLength(1);
      expect(writeCalls[0]?.[1]).toMatchObject({
        config: expect.objectContaining({ host: "10.0.0.3" }),
      });
    });
  });

  it("does not retry the same failed auto save endlessly", async () => {
    const invokeMock = vi.mocked(invoke);
    const config = { ...toPayload(EMPTY_FORM), host: "10.0.0.1" };

    invokeMock.mockImplementation(async (command) => {
      if (command === "read_proxy_config") {
        return { path: "/tmp/config.json", config };
      }
      if (command === "proxy_status") {
        return PROXY_STATUS;
      }
      if (command === "write_proxy_config") {
        throw new Error("disk full");
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(
      <I18nProvider>
        <ConfigScreen activeSectionId="core" />
      </I18nProvider>
    );

    const hostInput = screen.getByLabelText("host");
    await waitFor(() => {
      expect(hostInput).toHaveValue("10.0.0.1");
    });
    fireEvent.change(hostInput, { target: { value: "10.0.0.9" } });

    await waitForAutoSaveWindow();

    await waitFor(() => {
      expect(
        invokeMock.mock.calls.filter(([command]) => command === "write_proxy_config")
      ).toHaveLength(1);
    });

    await waitForAutoSaveWindow();

    expect(
      invokeMock.mock.calls.filter(([command]) => command === "write_proxy_config")
    ).toHaveLength(1);
    expect(screen.getByTestId("status-message")).toHaveTextContent("disk full");
  });
});
