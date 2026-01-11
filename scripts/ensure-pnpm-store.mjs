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

let storePath = "";
const errors = [];

for (const candidate of candidates) {
  try {
    const { stdout } = await execFileAsync(candidate.cmd, candidate.args);
    storePath = stdout.trim();
    if (storePath) break;
    errors.push(`${candidate.label}: empty stdout`);
  } catch (error) {
    errors.push(`${candidate.label}: ${(error && error.message) || "unknown error"}`);
  }
}

if (!storePath) {
  const details = errors.join(" | ");
  throw new Error(`Failed to resolve pnpm store path. Tried ${candidates.length} commands. Details: ${details}`);
}

await mkdir(storePath, { recursive: true });
