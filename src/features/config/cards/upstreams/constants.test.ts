import { describe, expect, it } from "vitest";

import { UPSTREAM_COLUMNS } from "@/features/config/cards/upstreams/constants";

describe("upstreams/constants", () => {
  it("adjusts id, provider, account, and priority column widths", () => {
    const idColumn = UPSTREAM_COLUMNS.find((column) => column.id === "id");
    const providerColumn = UPSTREAM_COLUMNS.find((column) => column.id === "provider");
    const accountColumn = UPSTREAM_COLUMNS.find((column) => column.id === "account");
    const priorityColumn = UPSTREAM_COLUMNS.find((column) => column.id === "priority");

    expect(idColumn?.headerClassName).toBe("w-[12rem]");
    expect(idColumn?.cellClassName).toBe("w-[12rem] max-w-[12rem]");
    expect(providerColumn?.headerClassName).toBe("w-[10rem]");
    expect(providerColumn?.cellClassName).toBe("w-[10rem] max-w-[10rem]");
    expect(accountColumn?.headerClassName).toBe("w-[7.5rem]");
    expect(accountColumn?.cellClassName).toBe("w-[7.5rem] max-w-[7.5rem]");
    expect(priorityColumn?.headerClassName).toBe("w-[6rem]");
    expect(priorityColumn?.cellClassName).toBe("w-[6rem]");
  });
});
