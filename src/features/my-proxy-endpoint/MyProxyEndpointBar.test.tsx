import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "@/lib/i18n";

import { MyProxyEndpointBar } from "./MyProxyEndpointBar";
import type { MyProxyEndpointSnapshot } from "./types";

const { readMock, selectMock } = vi.hoisted(() => ({
  readMock: vi.fn(),
  selectMock: vi.fn(),
}));

vi.mock("./api", () => ({
  readMyProxyEndpoint: readMock,
  selectMyProxyEndpoint: selectMock,
}));

const writeTextMock = vi.mocked(writeText);

const PLAIN_KEY = "sk-super-secret-key";

function snapshot(overrides: Partial<MyProxyEndpointSnapshot> = {}): MyProxyEndpointSnapshot {
  return {
    listenHost: "0.0.0.0",
    port: 9208,
    scanEnabled: true,
    effectiveIp: "192.168.1.23",
    effectiveFormat: "openai",
    availableIps: ["192.168.1.23", "10.0.0.5", "127.0.0.1"],
    apiKey: PLAIN_KEY,
    apiKeyConfigured: true,
    ...overrides,
  };
}

function renderBar() {
  return render(
    <I18nProvider>
      <MyProxyEndpointBar />
    </I18nProvider>
  );
}

describe("my-proxy-endpoint/MyProxyEndpointBar", () => {
  beforeEach(() => {
    readMock.mockReset();
    selectMock.mockReset();
    writeTextMock.mockReset();
    writeTextMock.mockResolvedValue(undefined);
    readMock.mockResolvedValue(snapshot());
  });

  afterEach(() => {
    cleanup();
  });

  it("renders the composed url once the snapshot resolves", async () => {
    renderBar();
    const bar = await screen.findByTestId("my-proxy-endpoint-bar");
    expect(bar).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId("my-proxy-endpoint-url")).toHaveTextContent(
        "http://192.168.1.23:9208/v1"
      );
    });
  });

  it("copies the url and the ip", async () => {
    const user = userEvent.setup();
    renderBar();
    await screen.findByTestId("my-proxy-endpoint-bar");

    await user.click(screen.getByTestId("my-proxy-endpoint-copy-url"));
    await waitFor(() =>
      expect(writeTextMock).toHaveBeenCalledWith("http://192.168.1.23:9208/v1")
    );

    await user.click(screen.getByTestId("my-proxy-endpoint-copy-ip"));
    await waitFor(() => expect(writeTextMock).toHaveBeenCalledWith("192.168.1.23"));
  });

  it("copies the key while never rendering it in the clear", async () => {
    const user = userEvent.setup();
    renderBar();
    await screen.findByTestId("my-proxy-endpoint-bar");

    expect(document.body.textContent).not.toContain(PLAIN_KEY);
    await user.click(screen.getByTestId("my-proxy-endpoint-copy-key"));
    await waitFor(() => expect(writeTextMock).toHaveBeenCalledWith(PLAIN_KEY));
    expect(document.body.textContent).not.toContain(PLAIN_KEY);
  });

  it("disables the key copy entry when no key is configured", async () => {
    readMock.mockResolvedValue(snapshot({ apiKey: null, apiKeyConfigured: false }));
    renderBar();
    await screen.findByTestId("my-proxy-endpoint-bar");
    expect(screen.getByTestId("my-proxy-endpoint-copy-key")).toBeDisabled();
    expect(screen.getByTestId("my-proxy-endpoint-key-mask")).not.toHaveTextContent("\u2022");
  });

  it("gates the ip select on loopback while keeping the format switch usable", async () => {
    readMock.mockResolvedValue(
      snapshot({
        listenHost: "127.0.0.1",
        scanEnabled: false,
        effectiveIp: "127.0.0.1",
        availableIps: [],
      })
    );
    renderBar();
    await screen.findByTestId("my-proxy-endpoint-bar");
    await waitFor(() => {
      expect(screen.getByTestId("my-proxy-endpoint-url")).toHaveTextContent(
        "http://127.0.0.1:9208/v1"
      );
    });
    expect(screen.getByTestId("my-proxy-endpoint-ip-trigger")).toBeDisabled();
    expect(screen.getByTestId("my-proxy-endpoint-format-trigger")).not.toBeDisabled();
  });

  it("persists a format switch and re-composes the url", async () => {
    const user = userEvent.setup();
    selectMock.mockResolvedValue(snapshot({ effectiveFormat: "gemini" }));
    renderBar();
    await screen.findByTestId("my-proxy-endpoint-bar");

    await user.click(screen.getByTestId("my-proxy-endpoint-format-trigger"));
    const option = await screen.findByRole("option", { name: /Gemini/ });
    await user.click(option);

    await waitFor(() =>
      expect(selectMock).toHaveBeenCalledWith("192.168.1.23", "gemini")
    );
    await waitFor(() => {
      expect(screen.getByTestId("my-proxy-endpoint-url")).toHaveTextContent(
        "http://192.168.1.23:9208/v1beta"
      );
    });
  });

  it("degrades silently when the snapshot payload is malformed", async () => {
    readMock.mockResolvedValue(undefined);
    renderBar();
    const bar = await screen.findByTestId("my-proxy-endpoint-bar");
    expect(bar).toBeInTheDocument();
    expect(screen.queryByTestId("my-proxy-endpoint-url")).toBeNull();
  });

  it("degrades silently when the snapshot command fails", async () => {
    readMock.mockRejectedValue(new Error("boom"));
    renderBar();
    const bar = await screen.findByTestId("my-proxy-endpoint-bar");
    expect(bar).toBeInTheDocument();
    expect(screen.queryByTestId("my-proxy-endpoint-url")).toBeNull();
  });
});
