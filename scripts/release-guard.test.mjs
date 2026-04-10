import assert from "node:assert/strict";
import test from "node:test";

import {
  evaluateReleaseGuard,
  parseReleaseCommitVersion,
} from "./release-guard.mjs";

test("parseReleaseCommitVersion 只接受 release 提交标题", () => {
  assert.equal(
    parseReleaseCommitVersion("chore: release v0.1.63 (#199)"),
    "0.1.63"
  );
  assert.equal(
    parseReleaseCommitVersion("fix stale gpt display-name test (#198)"),
    ""
  );
});

test("evaluateReleaseGuard 对真正的 release merge commit 放行", () => {
  assert.deepEqual(
    evaluateReleaseGuard({
      version: "0.1.63",
      newestTag: "v0.1.62",
      commitSubject: "chore: release v0.1.63 (#199)",
    }),
    {
      currentTag: "v0.1.63",
      isPrerelease: false,
      isRelease: true,
      releaseCommitVersion: "0.1.63",
    }
  );
});

test("evaluateReleaseGuard 跳过普通 main push，即使版本号还领先最新 tag", () => {
  assert.deepEqual(
    evaluateReleaseGuard({
      version: "0.1.62",
      newestTag: "v0.1.61",
      commitSubject: "fix stale gpt display-name test (#198)",
    }),
    {
      currentTag: "v0.1.62",
      isPrerelease: false,
      isRelease: false,
      releaseCommitVersion: "",
    }
  );
});

test("evaluateReleaseGuard 跳过标题和 package 版本不一致的提交", () => {
  assert.deepEqual(
    evaluateReleaseGuard({
      version: "0.1.63",
      newestTag: "v0.1.62",
      commitSubject: "chore: release v0.1.64 (#200)",
    }),
    {
      currentTag: "v0.1.63",
      isPrerelease: false,
      isRelease: false,
      releaseCommitVersion: "0.1.64",
    }
  );
});
