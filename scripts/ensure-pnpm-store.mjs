import { execFile } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { promisify } from "node:util";

// Ensure pnpm store path exists so setup-node cache doesn't fail when install is skipped.
const execFileAsync = promisify(execFile);

const candidates = [
  { cmd: "corepack", args: ["pnpm", "store", "path", "--silent"], label: "corepack pnpm" },
  { cmd: "pnpm", args: ["store", "path", "--silent"], label: "pnpm" },
  { cmd: "npx", args: ["-y", "pnpm", "store", "path", "--silent"], label: "npx pnpm" },
];

function getErrorMessage(error) {
  if (error instanceof Error) return error.message;
  return String(error);
}

function getErrorCode(error) {
  if (error && typeof error === "object" && "code" in error) {
    const code = error.code;
    return typeof code === "string" ? code : "";
  }
  return "";
}

let storePath = "";
const errors = [];
const errorCodes = [];

for (const candidate of candidates) {
  try {
    const { stdout } = await execFileAsync(candidate.cmd, candidate.args);
    storePath = stdout.trim();
    if (storePath) break;
    errors.push(`${candidate.label}: empty stdout`);
  } catch (error) {
    const message = getErrorMessage(error);
    const code = getErrorCode(error);
    if (code) errorCodes.push(code);
    errors.push(`${candidate.label}: ${message || "unknown error"}`);
  }
}

if (!storePath) {
  const details = errors.join(" | ");
  const allMissing =
    errors.length === candidates.length &&
    errorCodes.length === candidates.length &&
    errorCodes.every((code) => code === "ENOENT");
  if (allMissing) {
    console.warn(`pnpm not found; skipping pnpm store initialization. Details: ${details}`);
    process.exit(0);
  }
  throw new Error(`Failed to resolve pnpm store path. Tried ${candidates.length} commands. Details: ${details}`);
}

await mkdir(storePath, { recursive: true });
