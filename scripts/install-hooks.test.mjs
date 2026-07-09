import assert from "node:assert/strict";
import { mkdtemp, readFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createPreCommitHook, installPreCommitHook } from "./install-hooks.mjs";

test("pre-commit hook runs the skill release check", () => {
  assert.equal(
    createPreCommitHook(),
    `#!/bin/sh
set -eu

pixi run check-skills
`
  );
});

test("install hook writes an executable pre-commit hook", async () => {
  const root = await mkdtemp(join(tmpdir(), "general-agent-hooks-"));

  const hookPath = await installPreCommitHook(root);

  assert.equal(hookPath, join(root, ".git", "hooks", "pre-commit"));
  assert.equal(await readFile(hookPath, "utf8"), createPreCommitHook());
  assert.equal((await stat(hookPath)).mode & 0o111, 0o111);
});
