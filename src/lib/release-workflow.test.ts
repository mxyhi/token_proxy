import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const releaseWorkflow = readFileSync(".github/workflows/release.yml", "utf8");

describe("release workflow", () => {
  it("bootstraps the GitHub release before parallel asset uploads", () => {
    expect(releaseWorkflow).toContain("stable_release_bootstrap:");
    expect(releaseWorkflow).toContain("stable_release:\n    name: Release (${{ matrix.platform }} / ${{ matrix.target }})\n    needs: [stable_tag, stable_release_bootstrap]");
    expect(releaseWorkflow).toContain("stable_cli_release:\n    name: Release CLI (${{ matrix.platform }} / ${{ matrix.target }})\n    needs: [stable_tag, stable_release_bootstrap]");
  });

  it("does not rely on polling the release from CLI jobs", () => {
    expect(releaseWorkflow).not.toContain("- name: Wait for GitHub Release");
    expect(releaseWorkflow).not.toContain("uses: ./.github/actions/wait-for-release");
  });
});
