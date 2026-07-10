import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const projectRoot = resolve(process.env.PIXI_PROJECT_ROOT ?? process.cwd());
const androidRoot = resolve(projectRoot, "apps/android");
const androidSdk = resolve(projectRoot, ".tool/android-sdk");
const androidEnvironment = {
  ...process.env,
  ANDROID_HOME: androidSdk,
  ANDROID_SDK_ROOT: androidSdk,
};

const checks = [
  { command: "cargo", args: ["test", "-p", "agent-runtime"], cwd: projectRoot },
  { command: "cargo", args: ["test", "-p", "mobile-ffi"], cwd: projectRoot },
  {
    command: resolve(androidRoot, "gradlew"),
    args: [":app:testDebugUnitTest"],
    cwd: androidRoot,
    env: androidEnvironment,
  },
  {
    command: resolve(androidRoot, "gradlew"),
    args: [":app:assembleDebug"],
    cwd: androidRoot,
    env: androidEnvironment,
  },
];

for (const check of checks) {
  const label = `${check.command} ${check.args.join(" ")}`;
  console.log(`\n==> ${label}`);
  const result = spawnSync(check.command, check.args, {
    cwd: check.cwd,
    env: check.env ?? process.env,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
