import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const CODE_EXTENSIONS = new Set([
  ".rs", ".ts", ".tsx", ".js", ".mjs", ".kt", ".kts", ".css",
]);
const LINE_LIMIT = 1000;

export function countPhysicalLines(bytes) {
  if (bytes.length === 0) return 0;
  let lines = bytes[bytes.length - 1] === 0x0a ? 0 : 1;
  for (const byte of bytes) {
    if (byte === 0x0a) lines += 1;
  }
  return lines;
}

export function overBudgetEntries(entries) {
  return entries.filter((entry) => entry.lines >= LINE_LIMIT);
}

export function runSourceLineCheck(
  root = resolve(dirname(fileURLToPath(import.meta.url)), ".."),
) {
  const listing = spawnSync(
    "git",
    ["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "crates", "apps", "scripts"],
    { cwd: root, encoding: "utf8" },
  );
  if (listing.error) throw listing.error;
  if (listing.status !== 0) throw new Error("failed to list project source files");
  const files = listing.stdout
    .split("\0")
    .filter(Boolean)
    .filter(isCodeSource);
  if (files.length === 0) return [];

  const entries = files.map((path) => ({
    lines: countPhysicalLines(readFileSync(resolve(root, path))),
    path,
  }));
  const failures = overBudgetEntries(entries);
  for (const failure of failures) {
    console.error(`${failure.lines} ${failure.path}`);
  }
  return failures;
}

function isCodeSource(path) {
  const segments = path.split("/");
  return CODE_EXTENSIONS.has(extname(path))
    && !segments.some((segment) => ["node_modules", "build", "target"].includes(segment));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const failures = runSourceLineCheck();
    if (failures.length > 0) process.exit(1);
    console.log("Source line budget passed");
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
