import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { HttpStatusBadge, httpStatusTone } from "@/features/logs/HttpStatusBadge";

describe("httpStatusTone", () => {
  it.each([
    [200, "success"],
    [204, "success"],
    [301, "redirect"],
    [400, "client-error"],
    [429, "client-error"],
    [500, "server-error"],
    [503, "server-error"],
    [0, "unknown"],
    [101, "unknown"],
  ] as const)("maps %d to %s", (status, tone) => {
    expect(httpStatusTone(status)).toBe(tone);
  });
});

describe("HttpStatusBadge", () => {
  it("renders the status with its tone", () => {
    render(<HttpStatusBadge status={502} />);
    expect(screen.getByText("502")).toHaveAttribute("data-status-tone", "server-error");
  });
});
