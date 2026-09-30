import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { UPSTREAM_COLUMNS } from "@/features/config/cards/upstreams/constants";
import { UpstreamsTable } from "@/features/config/cards/upstreams/table";
import type { UpstreamForm } from "@/features/config/types";

const LONG_ID = "codex-account-with-a-very-long-upstream-id-for-tooltip";

afterEach(() => {
  cleanup();
});

function renderPriorityTable(onPriorityChange: (index: number, priority: string) => void) {
  render(
    <UpstreamsTable
      upstreams={[buildUpstream()]}
      columns={UPSTREAM_COLUMNS}
      showApiKeys={false}
      disableDelete={false}
      onEdit={() => undefined}
      onCopy={() => undefined}
      onToggleEnabled={() => undefined}
      onPriorityChange={onPriorityChange}
      onDelete={() => undefined}
    />
  );
  return screen.getByRole("textbox", { name: /priority of/i });
}

function buildUpstream(): UpstreamForm {
  return {
    id: LONG_ID,
    providers: ["codex"],
    baseUrl: "https://api.example.com/v1",
    apiKeys: "",
    filterPromptCacheRetention: false,
    filterSafetyIdentifier: false,
    useChatCompletionsForResponses: false,
    rewriteDeveloperRoleToSystem: false,
    accountId: "",
    preferredEndpoint: "",
    proxyUrl: "",
    priority: "10",
    enabled: true,
    availableModelsMode: "all",
    availableModels: [],
    modelMappings: [],
    convertFromMap: {},
    overrides: { header: [] },
  };
}

describe("upstreams/table", () => {
  it("shows tooltip for truncated id cells on hover", async () => {
    const user = userEvent.setup();

    render(
      <UpstreamsTable
        upstreams={[buildUpstream()]}
        columns={UPSTREAM_COLUMNS}
        showApiKeys={false}
        disableDelete={false}
        onEdit={() => undefined}
        onCopy={() => undefined}
        onToggleEnabled={() => undefined}
        onPriorityChange={() => undefined}
        onDelete={() => undefined}
      />
    );

    const idCell = screen.getByText(LONG_ID);
    await user.hover(idCell);
    expect(await screen.findByRole("tooltip")).toHaveTextContent(LONG_ID);
  });

  it("keeps the actions column pinned to the right", () => {
    render(
      <UpstreamsTable
        upstreams={[buildUpstream()]}
        columns={UPSTREAM_COLUMNS}
        showApiKeys={false}
        disableDelete={false}
        onEdit={() => undefined}
        onCopy={() => undefined}
        onToggleEnabled={() => undefined}
        onPriorityChange={() => undefined}
        onDelete={() => undefined}
      />
    );

    const header = screen.getByRole("columnheader", { name: "Actions" });
    const actionButton = screen.getByRole("button", {
      name: /edit channel/i,
    });
    const actionCell = actionButton.closest("td");

    expect(header).toHaveClass("sticky", "right-0");
    expect(actionCell).not.toBeNull();
    expect(actionCell).toHaveClass("sticky", "right-0");
  });

  it("disables delete for account-backed special upstream rows", () => {
    render(
      <UpstreamsTable
        upstreams={[buildUpstream()]}
        columns={UPSTREAM_COLUMNS}
        showApiKeys={false}
        disableDelete={false}
        isDeleteDisabled={(upstream) =>
          upstream.providers.length === 1 && upstream.providers[0] === "codex"
        }
        onEdit={() => undefined}
        onCopy={() => undefined}
        onToggleEnabled={() => undefined}
        onPriorityChange={() => undefined}
        onDelete={() => undefined}
      />
    );

    expect(
      screen.getByRole("button", { name: /delete channel/i })
    ).toBeDisabled();
  });

  it("disables copy for account-backed special upstream rows", () => {
    render(
      <UpstreamsTable
        upstreams={[buildUpstream()]}
        columns={UPSTREAM_COLUMNS}
        showApiKeys={false}
        disableDelete={false}
        isCopyDisabled={(upstream) =>
          upstream.providers.length === 1 && upstream.providers[0] === "codex"
        }
        onEdit={() => undefined}
        onCopy={() => undefined}
        onToggleEnabled={() => undefined}
        onPriorityChange={() => undefined}
        onDelete={() => undefined}
      />
    );

    expect(
      screen.getByRole("button", { name: /copy channel/i })
    ).toBeDisabled();
  });

  it("commits inline priority edits on Enter", async () => {
    const user = userEvent.setup();
    const onPriorityChange = vi.fn();
    const input = renderPriorityTable(onPriorityChange);

    expect(input).toHaveValue("10");
    await user.clear(input);
    await user.type(input, " 25 {Enter}");

    expect(onPriorityChange).toHaveBeenCalledExactlyOnceWith(0, "25");
    expect(input).not.toHaveFocus();
  });

  it("reverts invalid inline priority without committing", async () => {
    const user = userEvent.setup();
    const onPriorityChange = vi.fn();
    const input = renderPriorityTable(onPriorityChange);

    await user.clear(input);
    await user.type(input, "abc");
    await user.tab();

    expect(onPriorityChange).not.toHaveBeenCalled();
    expect(input).toHaveValue("10");
  });

  it("discards inline priority edits on Escape", async () => {
    const user = userEvent.setup();
    const onPriorityChange = vi.fn();
    const input = renderPriorityTable(onPriorityChange);

    await user.clear(input);
    await user.type(input, "99{Escape}");

    expect(onPriorityChange).not.toHaveBeenCalled();
    expect(input).toHaveValue("10");
  });
});
