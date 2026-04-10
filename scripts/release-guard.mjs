import fs from "node:fs";
import { execSync } from "node:child_process";
import { fileURLToPath } from "node:url";

function setOutput(key, value) {
  const outputPath = process.env.GITHUB_OUTPUT;
  const line = `${key}=${value}\n`;
  if (outputPath) {
    fs.appendFileSync(outputPath, line);
  } else {
    process.stdout.write(line);
  }
}

function readPackageVersion() {
  const pkg = JSON.parse(fs.readFileSync("package.json", "utf8"));
  return pkg.version;
}

function readHeadCommitSubject() {
  return execSync("git log -1 --pretty=%s", {
    encoding: "utf8",
  }).trim();
}

function latestTag() {
  const output = execSync(
    "git tag --list 'v*' --sort=-v:refname | grep -E '^v[0-9]+\\.[0-9]+\\.[0-9]+$' | head -n 1",
    {
      encoding: "utf8",
    }
  ).trim();
  return output || "";
}

export function parseReleaseCommitVersion(commitSubject) {
  const match = commitSubject.match(
    /^chore: release v(\d+\.\d+\.\d+)(?: \(\#\d+\))?$/
  );
  return match?.[1] ?? "";
}

export function evaluateReleaseGuard({ version, newestTag, commitSubject }) {
  const isPrerelease = version.includes("-");
  const currentTag = `v${version}`;
  const releaseCommitVersion = parseReleaseCommitVersion(commitSubject);
  const isReleaseCommit = releaseCommitVersion === version;
  const isNewRelease = !isPrerelease && isReleaseCommit && newestTag !== currentTag;
  return {
    currentTag,
    isPrerelease,
    isRelease: isNewRelease,
    releaseCommitVersion,
  };
}

function main() {
  const version = readPackageVersion();
  const newestTag = latestTag();
  const commitSubject = readHeadCommitSubject();
  const result = evaluateReleaseGuard({
    version,
    newestTag,
    commitSubject,
  });

  setOutput("version", version);
  setOutput("latest_tag", newestTag);
  setOutput("commit_subject", commitSubject);
  setOutput("release_commit_version", result.releaseCommitVersion);
  setOutput("is_release", result.isRelease ? "true" : "false");
  // For backward compatibility with prerelease job gating: skip prerelease when this is a new release commit.
  setOutput("skip", result.isRelease ? "true" : "false");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main();
}
