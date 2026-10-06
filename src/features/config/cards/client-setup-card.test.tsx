import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ClientSetupCard } from "./client-setup-card";
import type { ClientSetupInfo } from "./client-setup-state";
import { m } from "@/paraglide/messages.js";

function setup(
  keys: ClientSetupInfo["enabled_api_keys"],
  required = true,
): ClientSetupInfo {
  return {
    enabled_api_keys: keys,
    local_auth_required: required,
    proxy_http_base_url: "http://localhost:9208",
    claude_settings_path: "/tmp/claude/settings.json",
    claude_base_url: "http://localhost:9208",
    claude_model: "claude-test",
    claude_auth_token_configured: required,
    codex_config_path: "/tmp/codex/config.toml",
    codex_auth_path: "/tmp/codex/auth.json",
    codex_disable_response_storage: true,
    codex_model: "gpt-test",
    codex_model_provider: "token_proxy",
    codex_model_reasoning_effort: "high",
    codex_network_access: "enabled",
    codex_preferred_auth_method: "apikey",
    codex_provider_base_url: "http://localhost:9208/v1",
    codex_provider_name: "token_proxy",
    codex_provider_requires_openai_auth: true,
    codex_provider_wire_api: "responses",
    codex_api_key_configured: required,
  };
}

describe("client API key selection", () => {
  afterEach(cleanup);

  it("requires explicit selection for multiple keys and sends its ID to both clients", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "preview_client_setup"
        ? setup([
            { id: "a", name: "Alpha" },
            { id: "b", name: "Beta" },
          ])
        : { paths: ["/tmp/test"] },
    );
    render(<ClientSetupCard savedAt="" isDirty={false} />);
    await screen.findByRole("combobox");
    fireEvent.click(
      screen.getAllByRole("button", { name: m.common_show() })[0],
    );
    expect(
      screen.getByRole("button", { name: m.client_setup_apply() }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getAllByRole("button", { name: m.common_close() })[0],
    );
    await user.click(screen.getByRole("combobox"));
    await user.click(screen.getByRole("option", { name: "Beta" }));
    for (const [index, command] of [
      [0, "write_claude_code_settings"],
      [1, "write_codex_config"],
    ] as const) {
      fireEvent.click(
        screen.getAllByRole("button", { name: m.common_show() })[index],
      );
      fireEvent.click(
        screen.getByRole("button", { name: m.client_setup_apply() }),
      );
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith(command, { keyId: "b" }),
      );
      fireEvent.click(
        screen.getAllByRole("button", { name: m.common_close() })[0],
      );
    }
  });

  it("selects the only enabled key automatically and blocks all-disabled configurations", async () => {
    let preview = setup([{ id: "only", name: "Only" }]);
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "preview_client_setup" ? preview : { paths: [] },
    );
    const view = render(<ClientSetupCard savedAt="" isDirty={false} />);
    await screen.findByRole("combobox");
    fireEvent.click(
      screen.getAllByRole("button", { name: m.common_show() })[0],
    );
    fireEvent.click(
      screen.getByRole("button", { name: m.client_setup_apply() }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("write_claude_code_settings", {
        keyId: "only",
      }),
    );
    fireEvent.click(
      screen.getAllByRole("button", { name: m.common_close() })[0],
    );
    preview = setup([]);
    view.rerender(<ClientSetupCard savedAt="updated" isDirty={false} />);
    await screen.findByText(m.api_keys_all_disabled());
    fireEvent.click(
      screen.getAllByRole("button", { name: m.common_show() })[0],
    );
    expect(
      screen.getByRole("button", { name: m.client_setup_apply() }),
    ).toBeDisabled();
  });
});
