import { describe, expect, it } from "vitest";

import {
  createNativeInboundFormatSet,
  removeInboundFormatsInSet,
} from "@/features/config/inbound-formats";

describe("inbound-formats", () => {
  it("creates native inbound format set (trims + unions)", () => {
    const formats = createNativeInboundFormatSet([" openai ", "codex", "", "openai"]);

    expect(formats.has("openai_chat")).toBe(true);
    expect(formats.has("openai_responses")).toBe(true);
    expect(formats.has("gemini")).toBe(false);
  });

  it("removes inbound formats already supported natively", () => {
    const native = createNativeInboundFormatSet(["openai"]);
    const filtered = removeInboundFormatsInSet(["openai_chat", "gemini"], native);

    expect(filtered).toEqual(["gemini"]);
  });
});

