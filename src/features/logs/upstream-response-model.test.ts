import { describe, expect, it } from "vitest";

import { auditUpstreamResponseModel } from "@/features/logs/upstream-response-model";

describe("auditUpstreamResponseModel", () => {
  it("flags a real upstream swap", () => {
    expect(auditUpstreamResponseModel("gpt-6-astra", null, "gpt-5.6-luna")).toEqual({
      kind: "mismatch",
      responseModel: "gpt-5.6-luna",
    });
  });

  it("compares against the mapped model that was actually sent", () => {
    expect(auditUpstreamResponseModel("alias", "gpt-6-astra", "gpt-5.6-luna").kind).toBe(
      "mismatch",
    );
    expect(auditUpstreamResponseModel("alias", "gpt-5.6-luna", "gpt-5.6-luna").kind).toBe("same");
  });

  it("treats dated aliases as variants and grok build ids as the same model", () => {
    expect(
      auditUpstreamResponseModel("claude-sonnet-4", null, "claude-sonnet-4-20250514").kind,
    ).toBe("variant");
    expect(auditUpstreamResponseModel("grok-4.7", null, "grok-4.7-build").kind).toBe("same");
  });

  it("hides the line when the response has no model", () => {
    expect(auditUpstreamResponseModel("gpt-6-astra", null, "  ").kind).toBe("same");
  });
});
