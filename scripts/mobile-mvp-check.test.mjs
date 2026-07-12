import assert from "node:assert/strict";
import test from "node:test";

import {
  REQUIRED_APK_SKILL_ASSETS,
  assertRequiredApkAssets,
  mobileMvpTaskNames,
} from "./mobile-mvp-check.mjs";

test("mobile MVP runs the three authoritative pixi Android tasks", () => {
  assert.deepEqual(mobileMvpTaskNames(), [
    "android-native",
    "android-test",
    "android-assemble",
  ]);
});

test("mobile MVP requires stable manifest and lock APK assets", () => {
  assert.deepEqual(REQUIRED_APK_SKILL_ASSETS, [
    "assets/skills/skill-bundle.json",
    "assets/skills/skill-bundle.lock",
  ]);
  assert.doesNotThrow(() => assertRequiredApkAssets([
    "AndroidManifest.xml",
    ...REQUIRED_APK_SKILL_ASSETS,
  ]));
  assert.throws(
    () => assertRequiredApkAssets(["assets/skills/skill-bundle.json"]),
    /assets\/skills\/skill-bundle.lock/,
  );
});
