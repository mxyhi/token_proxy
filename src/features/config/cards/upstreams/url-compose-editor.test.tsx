import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { UrlComposeEditor } from "@/features/config/cards/upstreams/url-compose-editor";
import { m } from "@/paraglide/messages.js";
import { type UrlComposeConfig } from "@/features/config/types";

afterEach(cleanup);

type SetupOptions = {
  providers?: string[];
  baseUrl?: string;
  value?: UrlComposeConfig;
};

/** 受控组件需要状态回灌；Harness 持有 state，通过 render-prop 注入 setValue。 */
function setup({
  providers = ["openai", "anthropic"],
  baseUrl = "https://x.com",
  value = {},
}: SetupOptions = {}) {
  const onChange = vi.fn();

  function Harness(props: {
    children: (args: {
      value: UrlComposeConfig;
      setValue: (next: UrlComposeConfig) => void;
    }) => ReactNode;
  }) {
    const [state, setState] = useState<UrlComposeConfig>(value);
    return <>{props.children({ value: state, setValue: setState })}</>;
  }

  render(
    <Harness>
      {({ value, setValue }) => (
        <UrlComposeEditor
          providers={providers}
          baseUrl={baseUrl}
          value={value}
          onChange={(next) => {
            onChange(next);
            setValue(next);
          }}
        />
      )}
    </Harness>
  );

  return { onChange };
}

async function expand(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByText(m.url_compose_toggle()));
}

describe("config/url-compose-editor", () => {
  it("默认折叠并显示摘要，展开后出现已勾选接口的配置行", async () => {
    const user = userEvent.setup();
    setup({ providers: ["openai", "anthropic"] });

    // 折叠态：摘要可见（已勾选家族但未定制 → 均为默认后缀），行未渲染
    expect(screen.getByText(m.url_compose_summary_default())).toBeDefined();
    await expand(user);

    expect(screen.getByText(m.url_compose_family_openai())).toBeDefined();
    expect(screen.getByText(m.url_compose_family_anthropic())).toBeDefined();
    expect(screen.queryByText(m.url_compose_family_openai_response())).toBeNull();
  });

  it("摘要反映已配置数量", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["openai", "openai-response", "anthropic"],
      value: {
        openai: { prefix: "", suffix: "/v3/chat/completions" },
      },
    });
    expect(screen.getByText(m.url_compose_summary_configured({ count: 1 }))).toBeDefined();
    await expand(user);
  });

  it("后缀失焦自动补开头斜杠", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({ providers: ["openai"] });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1];
    await user.type(suffixInput, "v3/chat/completions");
    await user.tab();

    await waitFor(() => {
      expect((suffixInput as HTMLInputElement).value).toBe("/v3/chat/completions");
    });
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as { openai?: { suffix: string } };
      expect(lastCall.openai?.suffix).toBe("/v3/chat/completions");
    });
  });

  it("预览展示 base+prefix+版本段替换后的完整出站地址", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["anthropic"],
      value: { anthropic: { prefix: "/anthropic", suffix: "/v1/messages" } },
    });
    await expand(user);

    const preview = screen.getByText("https://x.com").parentElement;
    expect(preview?.textContent).toContain("/anthropic");
    expect(preview?.textContent).toContain("/v1/messages");
  });

  it("后缀为默认值时重置按钮禁用，修改后可重置", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai"],
      value: { openai: { prefix: "", suffix: "/v3/chat/completions" } },
    });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("/v3/chat/completions");

    const resetTitle = m.url_compose_reset_title({ default: "/v1/chat/completions" });
    const resetButton = screen.getByTitle(resetTitle);
    expect(resetButton.hasAttribute("disabled")).toBe(false);

    await user.click(resetButton);
    await waitFor(() => {
      expect(suffixInput.value).toBe("/v1/chat/completions");
    });
    await waitFor(() => {
      const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as { openai?: { suffix: string } };
      expect(lastCall.openai?.suffix).toBe("/v1/chat/completions");
    });
  });

  it("映射示例浮窗展示版本段替换结果", async () => {
    const user = userEvent.setup();
    setup({
      providers: ["anthropic"],
      value: { anthropic: { prefix: "/anthropic", suffix: "/v1/messages" } },
    });
    await expand(user);

    const mapButton = screen.getByRole("button", { name: m.url_compose_map_open() });
    await user.click(mapButton);

    await waitFor(() => {
      expect(
        screen.getByText(m.url_compose_map_title({ name: m.url_compose_family_anthropic() }))
      ).toBeDefined();
    });
    expect(screen.getAllByText("/v1/messages/count_tokens").length).toBeGreaterThan(0);
  });
});
