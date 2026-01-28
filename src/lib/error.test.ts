import { describe, expect, it } from "vitest";

import { parseError } from "@/lib/error";

describe("parseError", () => {
  it("returns message for Error instances", () => {
    expect(parseError(new Error("boom"))).toBe("boom");
  });

  it("stringifies non-Error values", () => {
    expect(parseError("plain")).toBe("plain");
    expect(parseError({ ok: true })).toBe("[object Object]");
  });
});

