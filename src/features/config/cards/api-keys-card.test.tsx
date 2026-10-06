import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ApiKeysCard } from "./api-keys-card";
import {
  createEmptyUpstream,
  EMPTY_FORM,
  toForm,
  toPayload,
  validate,
} from "../form";
import { createLocalApiKey } from "../local-api-keys";
import type { ConfigForm, LocalApiKey } from "../types";
import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

function Harness({ initial = EMPTY_FORM }: { initial?: ConfigForm }) {
  const [form, setForm] = useState(initial);
  return (
    <I18nProvider>
      <ApiKeysCard
        form={form}
        onChange={(patch) => setForm({ ...form, ...patch })}
      />
      <output data-testid="payload">{JSON.stringify(toPayload(form))}</output>
    </I18nProvider>
  );
}

function savedKeys(): LocalApiKey[] {
  const parsed: { local_api_keys: LocalApiKey[] } = JSON.parse(
    screen.getByTestId("payload").textContent ?? "{}",
  );
  return parsed.local_api_keys;
}

describe("API Key management", () => {
  afterEach(cleanup);

  it("creates, edits, copies, shows, disables, reloads and confirms deletion of the final key", async () => {
    const view = render(<Harness />);
    expect(screen.getByText(m.api_keys_empty())).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.api_keys_add() }));
    expect(screen.getByText(m.api_keys_editor_title_add())).toBeInTheDocument();
    const generated = screen.getByLabelText(m.api_keys_value());
    expect(generated.getAttribute("value")).toMatch(/^tp-[a-f0-9]{64}$/);
    fireEvent.change(screen.getByLabelText(m.api_keys_name()), {
      target: { value: "Laptop" },
    });
    fireEvent.change(generated, { target: { value: "custom-secret" } });
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    expect(savedKeys()[0]).toMatchObject({
      name: "Laptop",
      key: "custom-secret",
      enabled: true,
      scope: { type: "auto" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: m.upstreams_row_copy({ rowLabel: "Laptop" }) }),
    );
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("custom-secret"),
    );
    expect(screen.queryByText("custom-secret")).not.toBeInTheDocument();
    expect(screen.getByText("cus••••cret")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.upstreams_show_api_keys() }));
    expect(screen.getByText("custom-secret")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.upstreams_hide_api_keys() }));
    expect(screen.queryByText("custom-secret")).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: m.upstreams_row_edit({ rowLabel: "Laptop" }) }),
    );
    expect(screen.getByText(m.api_keys_editor_title())).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText(m.api_keys_name()), {
      target: { value: "Desktop" },
    });
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    fireEvent.click(
      screen.getByRole("button", { name: m.upstreams_row_disable({ rowLabel: "Desktop" }) }),
    );
    expect(savedKeys()[0].enabled).toBe(false);
    const reloaded = toForm({
      ...toPayload(EMPTY_FORM),
      local_api_keys: savedKeys(),
    });
    view.unmount();
    render(<Harness initial={reloaded} />);
    expect(screen.getByText("Desktop")).toBeInTheDocument();
    expect(screen.getByText(m.api_keys_all_disabled())).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: m.upstreams_row_delete({ rowLabel: "Desktop" }) }),
    );
    const dialog = screen.getByRole("alertdialog");
    expect(
      within(dialog).getByText(m.api_keys_delete_last()),
    ).toBeInTheDocument();
    fireEvent.click(
      within(dialog).getByRole("button", { name: m.common_delete() }),
    );
    expect(savedKeys()).toEqual([]);
  });

  it("requires manual selection, searches all credential kinds, rejects duplicate secrets and preserves invalid bindings", () => {
    const api = {
      ...createEmptyUpstream(),
      id: "api",
      providers: ["openai"],
      baseUrl: "https://example.test",
    };
    const account = {
      ...createEmptyUpstream(),
      id: "login",
      providers: ["codex"],
      accountId: "account.json",
    };
    const existing = {
      ...createLocalApiKey(),
      name: "Existing",
      key: "duplicate",
    };
    render(
      <Harness
        initial={{
          ...EMPTY_FORM,
          upstreams: [api, account],
          localApiKeys: [existing],
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: m.api_keys_add() }));
    fireEvent.change(screen.getByLabelText(m.api_keys_name()), {
      target: { value: "Restricted" },
    });
    fireEvent.change(screen.getByLabelText(m.api_keys_value()), {
      target: { value: "duplicate" },
    });
    expect(screen.getByText(m.api_keys_duplicate())).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: m.common_save() }),
    ).toBeDisabled();
    fireEvent.change(screen.getByLabelText(m.api_keys_value()), {
      target: { value: "unique" },
    });
    fireEvent.click(screen.getByRole("switch", { name: "Auto" }));
    expect(screen.getByText(m.api_keys_select_required())).toBeInTheDocument();
    // 表头全选 + 2 个渠道
    expect(screen.getAllByRole("checkbox")).toHaveLength(3);
    fireEvent.change(screen.getByLabelText(m.api_keys_search()), {
      target: { value: "codex" },
    });
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    fireEvent.click(screen.getByRole("checkbox", { name: m.api_keys_select_all() }));
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    expect(savedKeys()[1].scope).toEqual({
      type: "selected",
      upstream_ids: ["login"],
    });
    cleanup();
    const stale: LocalApiKey = {
      ...existing,
      scope: { type: "selected", upstream_ids: ["deleted"] },
    };
    render(<Harness initial={{ ...EMPTY_FORM, localApiKeys: [stale] }} />);
    expect(screen.getByText(m.api_keys_expired())).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: m.upstreams_row_edit({ rowLabel: "Existing" }) }),
    );
    expect(
      within(screen.getByRole("alertdialog")).getByText(/deleted/),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    expect(savedKeys()[0].scope).toEqual(stale.scope);
  });

  it("selects multiple account upstreams and preserves the exact selection after reload", () => {
    const upstreams = ["account-a", "account-b", "account-c"].map((id) => ({
      ...createEmptyUpstream(),
      id,
      providers: ["codex"],
      accountId: `${id}.json`,
    }));
    const initial = { ...EMPTY_FORM, upstreams };
    const view = render(<Harness initial={initial} />);
    fireEvent.click(screen.getByRole("button", { name: m.api_keys_add() }));
    fireEvent.change(screen.getByLabelText(m.api_keys_name()), {
      target: { value: "Two accounts" },
    });
    fireEvent.click(screen.getByRole("switch", { name: "Auto" }));
    fireEvent.click(screen.getByRole("checkbox", { name: /account-a/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /account-b/ }));
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    expect(savedKeys()[0].scope).toEqual({
      type: "selected",
      upstream_ids: ["account-a", "account-b"],
    });
    const reloaded = toForm({
      ...toPayload(initial),
      local_api_keys: savedKeys(),
    });
    view.unmount();
    render(<Harness initial={reloaded} />);
    fireEvent.click(
      screen.getByRole("button", { name: m.upstreams_row_edit({ rowLabel: "Two accounts" }) }),
    );
    expect(screen.getByRole("checkbox", { name: /account-a/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /account-b/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /account-c/ })).not.toBeChecked();
    fireEvent.click(screen.getByRole("checkbox", { name: /account-a/ }));
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    expect(savedKeys()[0].scope).toEqual({
      type: "selected",
      upstream_ids: ["account-b"],
    });
  });

  it("lists channels in the picker by priority descending, keeping config order on ties", () => {
    const upstreams = [
      { id: "low", priority: "1" },
      { id: "unset", priority: "" },
      { id: "high", priority: "10" },
      { id: "tie", priority: "1" },
    ].map(({ id, priority }) => ({
      ...createEmptyUpstream(),
      id,
      priority,
      enabled: true,
      providers: ["openai"],
    }));
    render(<Harness initial={{ ...EMPTY_FORM, upstreams }} />);
    fireEvent.click(screen.getByRole("button", { name: m.api_keys_add() }));
    fireEvent.click(screen.getByRole("switch", { name: "Auto" }));
    const ids = screen
      .getAllByRole("checkbox")
      .slice(1)
      .map((checkbox) => checkbox.closest("label")?.textContent ?? "");
    expect(ids.map((text) => text.split("openai")[0])).toEqual([
      "high",
      "low",
      "tie",
      "unset",
    ]);
  });

  it("shows per-key cumulative usage and marks unused keys", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([
      {
        keyId: "used",
        requests: 1234,
        totalTokens: 126_400,
        costNanoUsd: 2_130_000_000,
        lastUsedMs: Date.UTC(2026, 9, 5, 12, 0),
      },
    ]);
    const used = { ...createLocalApiKey(), id: "used", name: "Used" };
    const idle = { ...createLocalApiKey(), id: "idle", name: "Idle" };
    render(<Harness initial={{ ...EMPTY_FORM, localApiKeys: [used, idle] }} />);

    expect(invoke).toHaveBeenCalledWith("read_local_api_key_usage");
    expect(await screen.findByText("126.4K")).toBeInTheDocument();
    expect(
      screen.getByText(`$2.13 · ${m.api_keys_usage_requests({ count: "1,234" })}`),
    ).toBeInTheDocument();
    expect(screen.getAllByText("—")).toHaveLength(1);
    expect(screen.getByText(m.api_keys_never_used())).toBeInTheDocument();
  });

  it("form validation blocks empty selected scope and duplicates before auto save", () => {
    const key = { ...createLocalApiKey(), name: "Key" };
    expect(
      validate({ ...EMPTY_FORM, localApiKeys: [key, { ...key, id: "other" }] })
        .valid,
    ).toBe(false);
    expect(
      validate({
        ...EMPTY_FORM,
        localApiKeys: [
          { ...key, scope: { type: "selected", upstream_ids: [] } },
        ],
      }).valid,
    ).toBe(false);
    expect(
      validate({
        ...EMPTY_FORM,
        localApiKeys: [
          { ...key, scope: { type: "selected", upstream_ids: ["deleted"] } },
        ],
      }).valid,
    ).toBe(true);
  });
});
