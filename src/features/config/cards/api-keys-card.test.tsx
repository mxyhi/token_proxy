import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
  waitFor,
} from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
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
import { m } from "@/paraglide/messages.js";

function Harness({ initial = EMPTY_FORM }: { initial?: ConfigForm }) {
  const [form, setForm] = useState(initial);
  return (
    <>
      <ApiKeysCard
        form={form}
        onChange={(patch) => setForm({ ...form, ...patch })}
      />
      <output data-testid="payload">{JSON.stringify(toPayload(form))}</output>
    </>
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
    fireEvent.click(screen.getByText(m.api_keys_add()));
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
    fireEvent.click(screen.getByRole("button", { name: m.api_keys_copy() }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("custom-secret"),
    );
    fireEvent.click(screen.getByRole("button", { name: m.common_show() }));
    expect(screen.getByText("custom-secret")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.common_hide() }));
    expect(screen.queryByText("custom-secret")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.common_edit() }));
    fireEvent.change(screen.getByLabelText(m.api_keys_name()), {
      target: { value: "Desktop" },
    });
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    fireEvent.click(screen.getByRole("switch"));
    expect(savedKeys()[0].enabled).toBe(false);
    const reloaded = toForm({
      ...toPayload(EMPTY_FORM),
      local_api_keys: savedKeys(),
    });
    view.unmount();
    render(<Harness initial={reloaded} />);
    expect(screen.getByText("Desktop")).toBeInTheDocument();
    expect(screen.getByText(m.api_keys_all_disabled())).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.common_delete() }));
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
    fireEvent.click(screen.getByText(m.api_keys_add()));
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
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    fireEvent.change(screen.getByLabelText(m.api_keys_search()), {
      target: { value: "codex" },
    });
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    fireEvent.click(screen.getByText(m.api_keys_select_all()));
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
    fireEvent.click(screen.getByRole("button", { name: m.common_edit() }));
    expect(
      within(screen.getByRole("alertdialog")).getByText(/deleted/),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: m.common_save() }));
    expect(savedKeys()[0].scope).toEqual(stale.scope);
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
