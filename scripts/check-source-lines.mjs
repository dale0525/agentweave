import { spawnSync } from "node:child_process";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const CODE_EXTENSIONS = new Set([
  ".rs", ".ts", ".tsx", ".js", ".mjs", ".kt", ".kts", ".css",
]);
const LINE_LIMIT = 1000;

export function parseWcOutput(output) {
  return output
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => {
      const match = /^\s*(\d+)\s+(.+)$/.exec(line);
      if (!match) throw new Error(`unexpected wc output: ${line}`);
      return { lines: Number(match[1]), path: match[2] };
    })
    .filter((entry) => entry.path !== "total");
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

  const counted = spawnSync("wc", ["-l", ...files], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 4 * 1024 * 1024,
  });
  if (counted.error) throw counted.error;
  if (counted.status !== 0) throw new Error("failed to count project source lines");
  const failures = overBudgetEntries(parseWcOutput(counted.stdout));
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
