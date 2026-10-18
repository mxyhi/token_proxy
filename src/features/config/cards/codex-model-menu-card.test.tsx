import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { m } from "@/paraglide/messages.js";
import { CodexModelMenuCard } from "./codex-model-menu-card";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

function candidateList() {
  const list = document.querySelector<HTMLElement>('[data-slot="codex-model-candidates"]');
  if (!list) {
    throw new Error("candidate list not rendered");
  }
  return within(list);
}

describe("CodexModelMenuCard", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
  });

  afterEach(cleanup);

  it("lists channel candidates without official models and toggles them", async () => {
    invokeMock.mockResolvedValue(["qwen3.8-27b-uncensored", "gpt-5.5", "deepseek-v4"]);
    const onChange = vi.fn();
    render(<CodexModelMenuCard models={["custom-model"]} onChange={onChange} isDirty={false} />);

    await waitFor(() =>
      expect(candidateList().getByText("deepseek-v4")).toBeInTheDocument(),
    );
    expect(invokeMock).toHaveBeenCalledWith("read_codex_extra_model_candidates");
    expect(candidateList().queryByText("gpt-5.5")).not.toBeInTheDocument();
    // 已添加的手工模型也在列表中，便于直接取消勾选。
    expect(candidateList().getByText("custom-model")).toBeInTheDocument();

    fireEvent.click(candidateList().getByRole("checkbox", { name: "deepseek-v4" }));
    expect(onChange).toHaveBeenLastCalledWith(["custom-model", "deepseek-v4"]);

    fireEvent.click(candidateList().getByRole("checkbox", { name: "custom-model" }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it("filters candidates by search keyword", async () => {
    invokeMock.mockResolvedValue(["qwen3.8-27b-uncensored", "deepseek-v4"]);
    render(<CodexModelMenuCard models={[]} onChange={vi.fn()} isDirty={false} />);

    await waitFor(() =>
      expect(candidateList().getByText("deepseek-v4")).toBeInTheDocument(),
    );
    fireEvent.change(screen.getByLabelText(m.codex_model_menu_search_placeholder()), {
      target: { value: "QWEN" },
    });

    expect(candidateList().getByText("qwen3.8-27b-uncensored")).toBeInTheDocument();
    expect(candidateList().queryByText("deepseek-v4")).not.toBeInTheDocument();
  });

  it("refreshes model discovery before reloading candidates", async () => {
    render(<CodexModelMenuCard models={[]} onChange={vi.fn()} isDirty={false} />);
    await waitFor(() =>
      expect(candidateList().getByText(m.codex_model_menu_no_candidates())).toBeInTheDocument(),
    );

    invokeMock.mockImplementation(async (command) =>
      command === "read_codex_extra_model_candidates" ? ["deepseek-v4"] : undefined,
    );
    fireEvent.click(screen.getByRole("button", { name: m.codex_model_menu_refresh() }));

    await waitFor(() =>
      expect(candidateList().getByText("deepseek-v4")).toBeInTheDocument(),
    );
    expect(invokeMock).toHaveBeenCalledWith("refresh_dashboard_model_discovery");
  });

  it("appends Qwen once and disables the shortcut afterwards", () => {
    const onChange = vi.fn();
    const view = render(
      <CodexModelMenuCard models={[]} onChange={onChange} isDirty={false} />,
    );

    fireEvent.click(screen.getByRole("button", { name: m.codex_model_menu_add_qwen() }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(["qwen3.8-27b-uncensored"]);

    view.rerender(
      <CodexModelMenuCard
        models={["qwen3.8-27b-uncensored"]}
        onChange={onChange}
        isDirty={false}
      />,
    );
    expect(
      screen.getByRole("button", { name: m.codex_model_menu_add_qwen() }),
    ).toBeDisabled();
    expect(screen.getByRole("listitem")).toHaveTextContent("qwen3.8-27b-uncensored");

    // 手动输入同一 Qwen ID 也按重复处理，不会重复追加。
    fireEvent.change(screen.getByLabelText(m.codex_model_menu_input_label()), {
      target: { value: " qwen3.8-27b-uncensored " },
    });
    fireEvent.click(screen.getByRole("button", { name: m.codex_model_menu_add() }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(
      screen.getByText(
        m.codex_model_menu_duplicate({ model: "qwen3.8-27b-uncensored" }),
      ),
    ).toBeInTheDocument();
  });

  it("removes an extra model without touching official models", () => {
    const onChange = vi.fn();
    render(
      <CodexModelMenuCard
        models={["qwen3.8-27b-uncensored", "other-model"]}
        onChange={onChange}
        isDirty={false}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: m.codex_model_menu_remove({ model: "qwen3.8-27b-uncensored" }),
      }),
    );
    expect(onChange).toHaveBeenCalledWith(["other-model"]);
  });

  it("adds trimmed ids from the input and rejects duplicates", () => {
    const onChange = vi.fn();
    const view = render(
      <CodexModelMenuCard models={[]} onChange={onChange} isDirty={false} />,
    );

    const input = screen.getByLabelText(m.codex_model_menu_input_label());
    fireEvent.change(input, { target: { value: "  custom-model  " } });
    fireEvent.click(screen.getByRole("button", { name: m.codex_model_menu_add() }));
    expect(onChange).toHaveBeenCalledWith(["custom-model"]);

    onChange.mockClear();
    view.unmount();
    render(
      <CodexModelMenuCard models={["custom-model"]} onChange={onChange} isDirty={false} />,
    );
    const duplicateInput = screen.getByLabelText(m.codex_model_menu_input_label());
    fireEvent.change(duplicateInput, { target: { value: "custom-model" } });
    fireEvent.click(screen.getByRole("button", { name: m.codex_model_menu_add() }));
    expect(onChange).not.toHaveBeenCalled();
    expect(
      screen.getByText(m.codex_model_menu_duplicate({ model: "custom-model" })),
    ).toBeInTheDocument();
  });

  it("rejects official model ids as extra models", () => {
    const onChange = vi.fn();
    render(<CodexModelMenuCard models={[]} onChange={onChange} isDirty={false} />);

    fireEvent.change(screen.getByLabelText(m.codex_model_menu_input_label()), {
      target: { value: "gpt-5.5" },
    });
    fireEvent.click(screen.getByRole("button", { name: m.codex_model_menu_add() }));

    expect(onChange).not.toHaveBeenCalled();
    expect(
      screen.getByText(m.codex_model_menu_already_official({ model: "gpt-5.5" })),
    ).toBeInTheDocument();
  });

  it("shows the save-first hint while the form is dirty", () => {
    render(<CodexModelMenuCard models={[]} onChange={vi.fn()} isDirty />);

    expect(screen.getByText(m.codex_model_menu_save_first())).toBeInTheDocument();
    expect(screen.getByText(m.codex_model_menu_empty())).toBeInTheDocument();
  });
});
