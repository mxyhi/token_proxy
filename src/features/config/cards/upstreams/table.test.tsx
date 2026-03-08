import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { UPSTREAM_COLUMNS } from "@/features/config/cards/upstreams/constants";
import { UpstreamsTable } from "@/features/config/cards/upstreams/table";
import type { UpstreamForm } from "@/features/config/types";

const LONG_ID = "codex-account-with-a-very-long-upstream-id-for-tooltip";
const LONG_EMAIL = "very.long.codex.account.email.for.tooltip@example.com";

afterEach(() => {
  cleanup();
});

function buildUpstream(): UpstreamForm {
  return {
    id: LONG_ID,
    providers: ["codex"],
    baseUrl: "https://api.example.com/v1",
    apiKey: "",
    filterPromptCacheRetention: false,
    filterSafetyIdentifier: false,
    kiroAccountId: "",
    codexAccountId: "codex-1.json",
    antigravityAccountId: "",
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
        kiroAccounts={new Map()}
        codexAccounts={
          new Map([
            [
              "codex-1.json",
              {
                account_id: "codex-1.json",
                email: LONG_EMAIL,
                expires_at: null,
                status: "active",
              },
            ],
          ])
        }
        antigravityAccounts={new Map()}
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

  it("shows tooltip for truncated account cells on hover", async () => {
    const user = userEvent.setup();

    render(
      <UpstreamsTable
        upstreams={[buildUpstream()]}
        columns={UPSTREAM_COLUMNS}
        showApiKeys={false}
        kiroAccounts={new Map()}
        codexAccounts={
          new Map([
            [
              "codex-1.json",
              {
                account_id: "codex-1.json",
                email: LONG_EMAIL,
                expires_at: null,
                status: "active",
              },
            ],
          ])
        }
        antigravityAccounts={new Map()}
        disableDelete={false}
        onEdit={() => undefined}
        onCopy={() => undefined}
        onToggleEnabled={() => undefined}
        onDelete={() => undefined}
      />
    );

    const accountCell = screen.getByText(LONG_EMAIL);
    await user.hover(accountCell);
    expect(await screen.findByRole("tooltip")).toHaveTextContent(LONG_EMAIL);
  });
});
