import { chmod, mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

export function createPreCommitHook() {
  return `#!/bin/sh
set -eu

pixi run check-skills
`;
}

export async function installPreCommitHook(root = process.cwd()) {
  const hooksDir = join(root, ".git", "hooks");
  const hookPath = join(hooksDir, "pre-commit");

  await mkdir(hooksDir, { recursive: true });
  await writeFile(hookPath, createPreCommitHook(), "utf8");
  await chmod(hookPath, 0o755);

  return hookPath;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const hookPath = await installPreCommitHook();
  console.log(`Installed pre-commit hook: ${hookPath}`);
}
