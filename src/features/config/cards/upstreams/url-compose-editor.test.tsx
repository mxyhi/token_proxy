import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  computeMapPopupPosition,
  UrlComposeEditor,
} from "@/features/config/cards/upstreams/url-compose-editor";
import { m } from "@/paraglide/messages.js";
import { type UrlComposeConfig } from "@/features/config/types";

afterEach(cleanup);

type SetupOptions = {
  providers?: string[];
  baseUrl?: string;
  value?: UrlComposeConfig;
  prefillSuffixDefaults?: boolean;
};

/** 受控组件需要状态回灌；Harness 持有 state，通过 render-prop 注入 setValue。 */
function setup({
  providers = ["openai", "anthropic"],
  baseUrl = "https://x.com",
  value = {},
  prefillSuffixDefaults,
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
          prefillSuffixDefaults={prefillSuffixDefaults}
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

  it("前缀输入提示按家族展示示例：openai /openai，anthropic /anthropic", async () => {
    const user = userEvent.setup();
    setup({ providers: ["openai", "anthropic"] });
    await expand(user);

    const textboxes = screen.getAllByRole("textbox") as HTMLInputElement[];
    expect(textboxes[0].placeholder).toBe(
      m.url_compose_prefix_placeholder({ example: "/openai" })
    );
    expect(textboxes[2].placeholder).toBe(
      m.url_compose_prefix_placeholder({ example: "/anthropic" })
    );
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

  it("新建模式预填：已勾选家族后缀自动填入家族默认值", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({ providers: ["openai"], prefillSuffixDefaults: true });

    await waitFor(() => {
      const firstCall = onChange.mock.calls[0]?.[0] as
        | { openai?: { prefix: string; suffix: string } }
        | undefined;
      expect(firstCall?.openai).toEqual({ prefix: "", suffix: "/v1/chat/completions" });
    });

    await expand(user);
    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("/v1/chat/completions");
  });

  it("编辑模式不预填：未配置 url_compose 的既有渠道后缀保持为空", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({ providers: ["openai"] });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("新建模式手动清空后缀后不被回填", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai"],
      prefillSuffixDefaults: true,
      value: { openai: { prefix: "", suffix: "/v1/chat/completions" } },
    });
    await expand(user);

    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    await user.clear(suffixInput);
    await user.tab();

    await waitFor(() => {
      expect(suffixInput.value).toBe("");
    });
    // 清空后家族键被删除，预填 effect 不得再次写入默认值
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as {
      openai?: { suffix: string };
    };
    expect(lastCall.openai).toBeUndefined();
  });

  it("复制新建不覆盖已携带的组合配置，仅补缺失家族", async () => {
    const user = userEvent.setup();
    const { onChange } = setup({
      providers: ["openai", "anthropic"],
      prefillSuffixDefaults: true,
      value: { openai: { prefix: "/x", suffix: "/v9/chat/completions" } },
    });

    await waitFor(() => {
      const firstCall = onChange.mock.calls[0]?.[0] as
        | { openai?: { suffix: string }; anthropic?: { suffix: string } }
        | undefined;
      expect(firstCall?.anthropic).toEqual({ prefix: "", suffix: "/v1/messages" });
      expect(firstCall?.openai?.suffix).toBe("/v9/chat/completions");
    });

    await expand(user);
    const suffixInput = screen.getAllByRole("textbox")[1] as HTMLInputElement;
    expect(suffixInput.value).toBe("/v9/chat/completions");
  });
});

describe("config/url-compose-editor/computeMapPopupPosition", () => {
  const viewport = { width: 1000, height: 800 };

  it("常规位置：右缘对齐感叹号、默认向上展开", () => {
    const position = computeMapPopupPosition(
      { left: 600, right: 617, top: 300, bottom: 317 },
      400,
      200,
      viewport.width,
      viewport.height
    );
    // 右缘 = 锚点右缘 617 → left = 217；上缘 = 锚点上缘 300 - 高 200 - 间距 8
    expect(position).toEqual({ left: 217, top: 92 });
  });

  it("左侧越界：收敛到窗口左缘（8px 边距）", () => {
    const position = computeMapPopupPosition(
      { left: 100, right: 117, top: 300, bottom: 317 },
      400,
      200,
      viewport.width,
      viewport.height
    );
    expect(position.left).toBe(8);
    expect(position.top).toBe(92);
  });

  it("右侧越界：收敛到窗口右缘内侧（8px 边距）", () => {
    const position = computeMapPopupPosition(
      { left: 982, right: 999, top: 300, bottom: 317 },
      400,
      200,
      viewport.width,
      viewport.height
    );
    expect(position.left).toBe(1000 - 400 - 8);
  });

  it("上方放不下：改为向下展开（右下角贴近感叹号）", () => {
    const position = computeMapPopupPosition(
      { left: 600, right: 617, top: 30, bottom: 47 },
      400,
      200,
      viewport.width,
      viewport.height
    );
    // 上缘 = 锚点下缘 47 + 间距 8
    expect(position.top).toBe(55);
  });

  it("上下均放不下：收敛进窗口内", () => {
    const position = computeMapPopupPosition(
      { left: 600, right: 617, top: 10, bottom: 27 },
      400,
      780,
      viewport.width,
      viewport.height
    );
    expect(position.top).toBe(Math.max(8, 800 - 780 - 8));
  });
});
