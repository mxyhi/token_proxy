import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ProxyCoreCard } from "@/features/config/cards/proxy-core-card";
import { EMPTY_FORM } from "@/features/config/form";
import { m } from "@/paraglide/messages.js";

describe("ProxyCoreCard", () => {
  afterEach(() => {
    cleanup();
  });

  it("renders the disabled xAI X Search switch and emits a direct form patch", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();

    render(
      <ProxyCoreCard
        form={EMPTY_FORM}
        showLocalKey={false}
        onToggleLocalKey={vi.fn()}
        onChange={onChange}
        onResetHotModelMappings={vi.fn()}
        proxyService={{
          status: null,
          requestState: "idle",
          message: "",
          isDirty: false,
          onRefresh: vi.fn(),
          onStart: vi.fn(),
          onStop: vi.fn(),
          onRestart: vi.fn(),
          onReload: vi.fn(),
        }}
      />
    );

    const toggle = screen.getByRole("switch", {
      name: m.proxy_core_xai_inject_x_search_label(),
    });
    expect(toggle).not.toBeChecked();

    await user.click(toggle);

    expect(onChange).toHaveBeenCalledWith({ xaiInjectXSearch: true });
  });
});
