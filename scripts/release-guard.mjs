import fs from "node:fs";
import { execSync } from "node:child_process";

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

function latestTag() {
  const output = execSync("git tag --list 'v*' --sort=-v:refname | head -n 1", {
    encoding: "utf8",
  }).trim();
  return output || "";
}

const version = readPackageVersion();
const isPrerelease = version.includes("-");
const currentTag = `v${version}`;
const newestTag = latestTag();
const isNewRelease = !isPrerelease && newestTag !== currentTag;

setOutput("version", version);
setOutput("latest_tag", newestTag);
setOutput("is_release", isNewRelease ? "true" : "false");
// For backward compatibility with prerelease job gating: skip prerelease when this is a new release commit.
setOutput("skip", isNewRelease ? "true" : "false");
