import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const REQUIRED_APK_SKILL_ASSETS = [
  "assets/skills/skill-bundle.json",
  "assets/skills/skill-bundle.lock",
  "assets/agent-app/agent-app.lock.json",
];

export function mobileMvpTaskNames() {
  return ["android-native", "android-test", "android-assemble"];
}

export function assertRequiredApkAssets(entries) {
  const available = new Set(entries);
  for (const asset of REQUIRED_APK_SKILL_ASSETS) {
    if (!available.has(asset)) {
      throw new Error(`Android APK is missing required skill asset: ${asset}`);
    }
  }
}

export function runMobileMvpCheck(root = resolve(dirname(fileURLToPath(import.meta.url)), "..")) {
  for (const task of mobileMvpTaskNames()) {
    runChecked("pixi", ["run", task], root, `${task} failed`);
  }
  const apk = resolve(root, "apps/android/app/build/outputs/apk/debug/app-debug.apk");
  const listing = spawnSync("jar", ["tf", apk], {
    cwd: root,
    encoding: "utf8",
  });
  if (listing.error) throw listing.error;
  if (listing.status !== 0) {
    throw new Error("failed to inspect assembled Android APK");
  }
  assertRequiredApkAssets(listing.stdout.split(/\r?\n/).filter(Boolean));
  console.log(`Verified Android skill assets in ${apk}`);
}

function runChecked(command, args, cwd, message) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(message);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    runMobileMvpCheck();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
