import { cleanup, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AvailableModelsEditor } from "@/features/config/cards/upstreams/available-models-editor";
import { createEmptyUpstream } from "@/features/config/form";
import { m } from "@/paraglide/messages.js";

afterEach(cleanup);

describe("账户渠道模型刷新", () => {
  it.each(["kiro", "codex", "xai"])("%s 使用绑定账户刷新并保留已选模型", async (provider) => {
    const user = userEvent.setup();
    const draft = {
      ...createEmptyUpstream(),
      providers: [provider],
      accountId: "test-account",
      baseUrl: "",
      proxyUrl: "$app_proxy_url",
      availableModelsMode: "selected" as const,
      availableModels: ["saved-model"],
    };
    const onChangeDraft = vi.fn();
    // 仅模拟目录响应，确认刷新不会触发白名单保存。
    vi.mocked(invoke).mockResolvedValue(["live-model", "live-model"]);
    render(<AvailableModelsEditor draft={draft} onChangeDraft={onChangeDraft} />);

    const refresh = screen.getByRole("button", { name: m.available_models_sync() });
    expect(refresh).toBeEnabled();
    await user.click(refresh);

    expect(invoke).toHaveBeenCalledWith("fetch_upstream_models", {
      provider,
      baseUrl: "",
      apiKey: "",
      accountId: "test-account",
      proxyUrl: "$app_proxy_url",
    });
    expect(await screen.findByRole("checkbox", { name: "live-model" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "saved-model" })).toBeChecked();
    expect(onChangeDraft).not.toHaveBeenCalled();
  });

  it("未绑定账户时不能刷新", () => {
    const draft = {
      ...createEmptyUpstream(),
      providers: ["codex"],
      accountId: "",
      availableModelsMode: "selected" as const,
    };
    render(<AvailableModelsEditor draft={draft} onChangeDraft={vi.fn()} />);
    expect(screen.getByRole("button", { name: m.available_models_sync() })).toBeDisabled();
  });

  it("刷新失败时显示错误并保留原模型", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("模型目录返回 403"));
    const draft = {
      ...createEmptyUpstream(),
      providers: ["xai"],
      accountId: "test-account",
      availableModelsMode: "selected" as const,
      availableModels: ["saved-model"],
    };
    const onChangeDraft = vi.fn();
    render(<AvailableModelsEditor draft={draft} onChangeDraft={onChangeDraft} />);
    await userEvent.click(screen.getByRole("button", { name: m.available_models_sync() }));
    expect(await screen.findByText("Error: 模型目录返回 403")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "saved-model" })).toBeChecked();
    expect(onChangeDraft).not.toHaveBeenCalled();
  });
});
