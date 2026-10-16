import { describe, expect, it } from "vitest";

import { summarizeAttemptError } from "@/features/logs/AttemptTrace";

describe("logs/AttemptTrace summarizeAttemptError", () => {
  it("extracts the upstream error message from a JSON body", () => {
    expect(
      summarizeAttemptError('HTTP 503: {"error":{"message":"No available accounts","type":"api_error"}}'),
    ).toBe("No available accounts");
    expect(summarizeAttemptError('HTTP 404: {"message":"Not Found","code":404}')).toBe("Not Found");
  });

  it("falls back to the raw text for truncated JSON or plain errors", () => {
    expect(summarizeAttemptError('HTTP 503: {"error":{"message":"No avail')).toBe(
      'HTTP 503: {"error":{"message":"No avail',
    );
    expect(summarizeAttemptError("client disconnected before completion")).toBe(
      "client disconnected before completion",
    );
    expect(summarizeAttemptError("  ")).toBeNull();
    expect(summarizeAttemptError(null)).toBeNull();
  });
});
