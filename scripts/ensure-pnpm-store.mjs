import { execFile } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { promisify } from "node:util";

// Ensure pnpm store path exists so setup-node cache doesn't fail when install is skipped.
const execFileAsync = promisify(execFile);
// Use corepack to avoid pnpm not being on PATH (e.g., Windows runners).
const { stdout } = await execFileAsync("corepack", [
  "pnpm",
  "store",
  "path",
  "--silent",
]);
const storePath = stdout.trim();

if (!storePath) {
  throw new Error("pnpm store path is empty.");
}

await mkdir(storePath, { recursive: true });
