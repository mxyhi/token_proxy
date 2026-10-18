import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { m } from "@/paraglide/messages.js";
import { CodexModelMenuCard } from "./codex-model-menu-card";

describe("CodexModelMenuCard", () => {
  afterEach(cleanup);

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
    expect(screen.getByText("qwen3.8-27b-uncensored")).toBeInTheDocument();

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
