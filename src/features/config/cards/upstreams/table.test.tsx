import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { UPSTREAM_COLUMNS } from "@/features/config/cards/upstreams/constants";
import { UpstreamsTable } from "@/features/config/cards/upstreams/table";
import type { UpstreamForm } from "@/features/config/types";

const LONG_ID = "codex-account-with-a-very-long-upstream-id-for-tooltip";

afterEach(() => {
  cleanup();
});

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
    preferredEndpoint: "",
    proxyUrl: "",
    priority: "10",
    enabled: true,
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
        onDelete={() => undefined}
      />
    );

    const header = screen.getByRole("columnheader", { name: "Actions" });
    const actionButton = screen.getByRole("button", {
      name: /edit upstream/i,
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
        onDelete={() => undefined}
      />
    );

    expect(
      screen.getByRole("button", { name: /delete upstream/i })
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
        onDelete={() => undefined}
      />
    );

    expect(
      screen.getByRole("button", { name: /copy upstream/i })
    ).toBeDisabled();
  });
});
