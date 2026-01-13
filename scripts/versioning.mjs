import fs from "node:fs";
import path from "node:path";
import { execSync } from "node:child_process";

const command = process.argv[2];
const inputVersion = process.argv[3];

if (!command) {
  console.error("Missing command. Use: prerelease | release");
  process.exit(1);
}

const ROOT = process.cwd();
const PATHS = {
  packageJson: path.join(ROOT, "package.json"),
  tauriConf: path.join(ROOT, "src-tauri", "tauri.conf.json"),
  cargoToml: path.join(ROOT, "src-tauri", "Cargo.toml"),
  cargoLock: path.join(ROOT, "src-tauri", "Cargo.lock"),
};

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function ensureSingleTrailingNewline(text) {
  const normalized = text.replace(/\r\n/g, "\n");
  const withoutTrailingBlankLines = normalized.replace(/(?:\n[ \t]*)+$/u, "");
  return `${withoutTrailingBlankLines}\n`;
}

function writeText(filePath, content) {
  fs.writeFileSync(filePath, ensureSingleTrailingNewline(content));
}

function writeJson(filePath, data) {
  writeText(filePath, JSON.stringify(data, null, 2));
}

function parseCoreVersion(version) {
  const [core] = version.split("-", 1);
  const parts = core.split(".").map((value) => Number(value));
  if (parts.length !== 3 || parts.some((value) => Number.isNaN(value))) {
    throw new Error(`Invalid version: ${version}`);
  }
  const [major, minor, patch] = parts;
  return { major, minor, patch };
}

function compareCore(a, b) {
  if (a.major !== b.major) return a.major - b.major;
  if (a.minor !== b.minor) return a.minor - b.minor;
  return a.patch - b.patch;
}

function getCurrentVersion() {
  return readJson(PATHS.packageJson).version;
}

function getTags(pattern) {
  const output = execSync(`git tag --list "${pattern}"`, { encoding: "utf8" })
    .trim();
  if (!output) return [];
  return output.split(/\r?\n/).filter(Boolean);
}

function computeNextPatch(baseVersion) {
  const core = parseCoreVersion(baseVersion);
  return `${core.major}.${core.minor}.${core.patch + 1}`;
}

function computeNextPrerelease(nextPatch) {
  // 通过已有 tag 推导预发布计数，避免重复发布。
  const tags = getTags(`v${nextPatch}-*`);
  const numbers = tags
    .map((tag) => tag.replace(`v${nextPatch}-`, ""))
    .map((value) => Number(value))
    .filter((value) => Number.isFinite(value));
  const next = numbers.length > 0 ? Math.max(...numbers) + 1 : 1;
  if (next > 65535) {
    throw new Error(
      "Prerelease identifier must be numeric-only and <= 65535 for MSI targets."
    );
  }
  const version = `${nextPatch}-${next}`;
  return { version, tag: `v${version}` };
}

function updateCargoToml(content, version) {
  const lines = content.split(/\r?\n/);
  let inPackage = false;
  let updated = false;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (/^\s*\[package\]\s*$/.test(line)) {
      inPackage = true;
      continue;
    }
    if (inPackage && /^\s*\[/.test(line)) {
      inPackage = false;
    }
    if (inPackage && /^\s*version\s*=/.test(line)) {
      lines[i] = `version = "${version}"`;
      updated = true;
      break;
    }
  }

  if (!updated) {
    throw new Error("Failed to update version in Cargo.toml");
  }

  return lines.join("\n");
}

function updateCargoLock(content, version) {
  const lines = content.split(/\r?\n/);
  let inPackage = false;
  let updated = false;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (line.startsWith("[[package]]")) {
      inPackage = false;
      continue;
    }
    if (/^name = "token_proxy"$/.test(line)) {
      inPackage = true;
      continue;
    }
    if (inPackage && /^version = ".*"$/.test(line)) {
      lines[i] = `version = "${version}"`;
      updated = true;
      break;
    }
  }

  if (!updated) {
    throw new Error("Failed to update version in Cargo.lock");
  }

  return lines.join("\n");
}

function applyVersion(version) {
  // 统一更新多个版本文件，确保构建产物版本一致。
  const packageJson = readJson(PATHS.packageJson);
  packageJson.version = version;
  writeJson(PATHS.packageJson, packageJson);

  const tauriConf = readJson(PATHS.tauriConf);
  tauriConf.version = version;
  writeJson(PATHS.tauriConf, tauriConf);

  const cargoToml = fs.readFileSync(PATHS.cargoToml, "utf8");
  writeText(PATHS.cargoToml, updateCargoToml(cargoToml, version));

  const cargoLock = fs.readFileSync(PATHS.cargoLock, "utf8");
  writeText(PATHS.cargoLock, updateCargoLock(cargoLock, version));
}

function setOutput(key, value) {
  const outputPath = process.env.GITHUB_OUTPUT;
  const line = `${key}=${value}\n`;
  if (outputPath) {
    fs.appendFileSync(outputPath, line);
  } else {
    process.stdout.write(line);
  }
}

function assertValidReleaseVersion(version) {
  // 手动发布：校验格式、递增性与 tag 唯一性。
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`Release version must be x.y.z, got: ${version}`);
  }
  const currentVersion = getCurrentVersion();
  const currentCore = parseCoreVersion(currentVersion);
  const nextCore = parseCoreVersion(version);
  if (compareCore(nextCore, currentCore) <= 0) {
    throw new Error(
      `Release version must be greater than ${currentCore.major}.${currentCore.minor}.${currentCore.patch}`
    );
  }
  const existingTags = getTags(`v${version}`);
  if (existingTags.length > 0) {
    throw new Error(`Tag v${version} already exists`);
  }
}

if (command === "prerelease") {
  const currentVersion = getCurrentVersion();
  const nextPatch = computeNextPatch(currentVersion);
  const { version, tag } = computeNextPrerelease(nextPatch);
  applyVersion(version);
  setOutput("version", version);
  setOutput("tag", tag);
} else if (command === "release") {
  if (!inputVersion) {
    console.error("Missing release version input");
    process.exit(1);
  }
  assertValidReleaseVersion(inputVersion);
  applyVersion(inputVersion);
  setOutput("version", inputVersion);
  setOutput("tag", `v${inputVersion}`);
} else {
  console.error(`Unknown command: ${command}`);
  process.exit(1);
}
